use anyhow::{Context, Result, anyhow, bail};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

struct HlslPreprocessor {
    macros: HashMap<String, String>,
    include_stack: Vec<PathBuf>,
    included_once: HashSet<PathBuf>,
}

#[derive(Clone, Copy)]
struct Conditional {
    parent_active: bool,
    active: bool,
    branch_taken: bool,
    saw_else: bool,
}

impl HlslPreprocessor {
    fn new() -> Self {
        Self {
            macros: HashMap::new(),
            include_stack: Vec::new(),
            included_once: HashSet::new(),
        }
    }

    fn process_file(&mut self, path: &Path) -> Result<String> {
        let path = canonical_or_absolute(path)?;
        if self.included_once.contains(&path) {
            return Ok(String::new());
        }
        if self.include_stack.contains(&path) {
            let chain = self
                .include_stack
                .iter()
                .chain(std::iter::once(&path))
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(" -> ");
            bail!("cyclic HLSL include: {chain}");
        }
        let source = fs::read_to_string(&path)
            .with_context(|| format!("failed to read HLSL include {}", path.display()))?;
        self.include_stack.push(path.clone());
        let result = self.process_source(&source, &path);
        self.include_stack.pop();
        result
    }

    fn process_source(&mut self, source: &str, path: &Path) -> Result<String> {
        let mut output = String::new();
        let mut conditionals = Vec::<Conditional>::new();
        let mut in_block_comment = false;
        for (line_index, raw_line) in source.lines().enumerate() {
            let line_number = line_index + 1;
            if let Some(directive) = directive_for_line(raw_line, &mut in_block_comment) {
                let result =
                    self.process_directive(directive.trim(), path, &mut conditionals, &mut output);
                result.with_context(|| format!("{}:{line_number}", path.display()))?;
            } else if conditionals.last().map_or(true, |branch| branch.active) {
                output.push_str(raw_line);
                output.push('\n');
            } else {
                output.push('\n');
            }
        }
        if !conditionals.is_empty() {
            bail!(
                "{}: unterminated conditional compilation block",
                path.display()
            );
        }
        Ok(output)
    }

    fn process_directive(
        &mut self,
        directive: &str,
        path: &Path,
        conditionals: &mut Vec<Conditional>,
        output: &mut String,
    ) -> Result<()> {
        let (command, argument) = directive
            .split_once(char::is_whitespace)
            .map_or((directive, ""), |(command, argument)| {
                (command, argument.trim())
            });
        let active = conditionals.last().map_or(true, |branch| branch.active);
        match command {
            "if" | "ifdef" | "ifndef" => {
                let parent_active = active;
                let branch_enabled = if parent_active {
                    match command {
                        "if" => evaluate_condition(argument, &self.macros)?,
                        "ifdef" => self.macros.contains_key(parse_identifier(argument)?),
                        _ => !self.macros.contains_key(parse_identifier(argument)?),
                    }
                } else {
                    false
                };
                conditionals.push(Conditional {
                    parent_active,
                    active: parent_active && branch_enabled,
                    branch_taken: parent_active && branch_enabled,
                    saw_else: false,
                });
            }
            "elif" => {
                let branch = conditionals
                    .last_mut()
                    .ok_or_else(|| anyhow!("#elif without a matching #if"))?;
                if branch.saw_else {
                    bail!("#elif cannot follow #else");
                }
                let selected = branch.parent_active
                    && !branch.branch_taken
                    && evaluate_condition(argument, &self.macros)?;
                branch.active = selected;
                branch.branch_taken |= selected;
            }
            "else" => {
                let branch = conditionals
                    .last_mut()
                    .ok_or_else(|| anyhow!("#else without a matching #if"))?;
                if branch.saw_else {
                    bail!("duplicate #else");
                }
                branch.saw_else = true;
                branch.active = branch.parent_active && !branch.branch_taken;
                branch.branch_taken |= branch.active;
            }
            "endif" => {
                conditionals
                    .pop()
                    .ok_or_else(|| anyhow!("#endif without a matching #if"))?;
            }
            "define" if active => {
                let (name, value) = split_macro_definition(argument)?;
                self.macros.insert(name.to_owned(), value.to_owned());
                output.push_str("#define ");
                output.push_str(argument);
                output.push('\n');
            }
            "undef" if active => {
                let name = parse_identifier(argument)?;
                self.macros.remove(name);
                output.push_str("#undef ");
                output.push_str(name);
                output.push('\n');
            }
            "include" if active => {
                let include_name = parse_include(argument)?;
                let include_path = path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(include_name);
                let expanded = self.process_file(&include_path)?;
                output.push_str(&expanded);
            }
            "pragma" if active && argument == "once" => {
                self.included_once.insert(canonical_or_absolute(path)?);
            }
            "error" if active => {
                bail!("#error {}", argument);
            }
            _ if !active => {}
            "" => {}
            _ => {
                output.push('#');
                output.push_str(directive);
                output.push('\n');
            }
        }
        Ok(())
    }
}

fn directive_for_line<'a>(line: &'a str, in_block_comment: &mut bool) -> Option<&'a str> {
    let bytes = line.as_bytes();
    let mut offset = 0;
    let mut found_directive = None;
    let mut saw_token = false;
    while offset < bytes.len() {
        if *in_block_comment {
            if bytes.get(offset..offset + 2) == Some(b"*/") {
                *in_block_comment = false;
                offset += 2;
            } else {
                offset += 1;
            }
            continue;
        }
        if bytes.get(offset..offset + 2) == Some(b"//") {
            break;
        }
        if bytes.get(offset..offset + 2) == Some(b"/*") {
            *in_block_comment = true;
            offset += 2;
            continue;
        }
        match bytes[offset] {
            b'"' | b'\'' => {
                let quote = bytes[offset];
                saw_token = true;
                offset += 1;
                while offset < bytes.len() {
                    if bytes[offset] == b'\\' {
                        offset = (offset + 2).min(bytes.len());
                    } else if bytes[offset] == quote {
                        offset += 1;
                        break;
                    } else {
                        offset += 1;
                    }
                }
            }
            b'#' if !saw_token => {
                found_directive = Some(offset + 1);
                saw_token = true;
                offset += 1;
            }
            byte if byte.is_ascii_whitespace() => offset += 1,
            _ => {
                saw_token = true;
                offset += 1;
            }
        }
    }
    found_directive.map(|start| &line[start..])
}

pub(crate) fn expand_source(source: &str, source_path: &Path) -> Result<String> {
    let path = canonical_or_absolute(source_path)?;
    let mut preprocessor = HlslPreprocessor::new();
    preprocessor.include_stack.push(path.clone());
    let result = preprocessor.process_source(source, &path);
    preprocessor.include_stack.pop();
    result
}

#[cfg(test)]
pub(crate) fn expand_file(path: &Path) -> Result<String> {
    HlslPreprocessor::new().process_file(path)
}

fn canonical_or_absolute(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .with_context(|| format!("failed to resolve {}", path.display()))?
    };
    if absolute.exists() {
        absolute
            .canonicalize()
            .with_context(|| format!("failed to canonicalize {}", absolute.display()))
    } else {
        Ok(absolute)
    }
}

fn parse_identifier(value: &str) -> Result<&str> {
    let identifier = value.trim();
    if identifier.is_empty()
        || !identifier.chars().enumerate().all(|(index, character)| {
            character == '_'
                || character.is_ascii_alphanumeric() && (index > 0 || !character.is_ascii_digit())
        })
    {
        bail!("expected a preprocessor identifier, got '{value}'");
    }
    Ok(identifier)
}

fn split_macro_definition(value: &str) -> Result<(&str, &str)> {
    let name_end = value
        .find(|character: char| character.is_whitespace() || character == '(')
        .unwrap_or(value.len());
    let name = &value[..name_end];
    parse_identifier(name)?;
    let tail = &value[name_end..];
    if tail.starts_with('(') {
        Ok((name, "1"))
    } else {
        Ok((
            name,
            if tail.trim().is_empty() {
                "1"
            } else {
                tail.trim()
            },
        ))
    }
}

fn parse_include(value: &str) -> Result<&str> {
    let value = value.trim();
    if value.len() < 2
        || !((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('<') && value.ends_with('>')))
    {
        bail!("expected a quoted or bracketed HLSL include path: {value}");
    }
    Ok(&value[1..value.len() - 1])
}

fn evaluate_condition(expression: &str, macros: &HashMap<String, String>) -> Result<bool> {
    let mut parser = ConditionParser {
        source: expression,
        offset: 0,
        macros,
    };
    let value = parser.parse_or()?;
    parser.skip_whitespace();
    if parser.offset != parser.source.len() {
        bail!(
            "unsupported token in HLSL conditional expression '{}'",
            &parser.source[parser.offset..]
        );
    }
    Ok(value != 0)
}

struct ConditionParser<'a> {
    source: &'a str,
    offset: usize,
    macros: &'a HashMap<String, String>,
}

impl ConditionParser<'_> {
    fn parse_or(&mut self) -> Result<i64> {
        let mut value = self.parse_and()?;
        loop {
            self.skip_whitespace();
            if !self.consume("||") {
                break;
            }
            let rhs = self.parse_and()?;
            value = bool_value(value != 0 || rhs != 0);
        }
        Ok(value)
    }

    fn parse_and(&mut self) -> Result<i64> {
        let mut value = self.parse_equality()?;
        loop {
            self.skip_whitespace();
            if !self.consume("&&") {
                break;
            }
            let rhs = self.parse_equality()?;
            value = bool_value(value != 0 && rhs != 0);
        }
        Ok(value)
    }

    fn parse_equality(&mut self) -> Result<i64> {
        let mut value = self.parse_relational()?;
        loop {
            self.skip_whitespace();
            if self.consume("==") {
                value = bool_value(value == self.parse_relational()?);
            } else if self.consume("!=") {
                value = bool_value(value != self.parse_relational()?);
            } else {
                break;
            }
        }
        Ok(value)
    }

    fn parse_relational(&mut self) -> Result<i64> {
        let mut value = self.parse_unary()?;
        loop {
            self.skip_whitespace();
            if self.consume("<=") {
                value = bool_value(value <= self.parse_unary()?);
            } else if self.consume(">=") {
                value = bool_value(value >= self.parse_unary()?);
            } else if self.consume("<") {
                value = bool_value(value < self.parse_unary()?);
            } else if self.consume(">") {
                value = bool_value(value > self.parse_unary()?);
            } else {
                break;
            }
        }
        Ok(value)
    }

    fn parse_unary(&mut self) -> Result<i64> {
        self.skip_whitespace();
        if self.consume("!") {
            return Ok(bool_value(self.parse_unary()? == 0));
        }
        if self.consume("-") {
            return Ok(-self.parse_unary()?);
        }
        if self.consume("+") {
            return self.parse_unary();
        }
        if self.consume("(") {
            let value = self.parse_or()?;
            self.skip_whitespace();
            if !self.consume(")") {
                bail!("missing ')' in HLSL conditional expression");
            }
            return Ok(value);
        }
        if self.consume_word("defined") {
            self.skip_whitespace();
            let parenthesized = self.consume("(");
            let name = self.parse_word()?;
            if parenthesized {
                self.skip_whitespace();
                if !self.consume(")") {
                    bail!("missing ')' after defined in HLSL conditional expression");
                }
            }
            return Ok(if self.macros.contains_key(&name) {
                1
            } else {
                0
            });
        }
        let token = self.parse_word_or_number()?;
        if let Ok(value) = parse_integer(&token) {
            return Ok(value);
        }
        let Some(value) = self.macros.get(&token) else {
            return Ok(0);
        };
        if value == &token {
            bail!("recursive macro in HLSL conditional expression: {token}");
        }
        if let Ok(value) = parse_integer(value) {
            return Ok(value);
        }
        evaluate_condition(value, self.macros).map(bool_value)
    }

    fn parse_word(&mut self) -> Result<String> {
        self.parse_word_or_number()
    }

    fn parse_word_or_number(&mut self) -> Result<String> {
        self.skip_whitespace();
        let start = self.offset;
        while let Some(character) = self.source[self.offset..].chars().next() {
            if !character.is_ascii_alphanumeric() && character != '_' {
                break;
            }
            self.offset += character.len_utf8();
        }
        if start == self.offset {
            bail!("expected a value in HLSL conditional expression");
        }
        Ok(self.source[start..self.offset].to_owned())
    }

    fn consume_word(&mut self, word: &str) -> bool {
        self.skip_whitespace();
        let rest = &self.source[self.offset..];
        if !rest.starts_with(word) {
            return false;
        }
        let next_is_boundary = rest[word.len()..].chars().next().map_or(true, |character| {
            !character.is_ascii_alphanumeric() && character != '_'
        });
        if next_is_boundary {
            self.offset += word.len();
            true
        } else {
            false
        }
    }

    fn consume(&mut self, token: &str) -> bool {
        if self.source[self.offset..].starts_with(token) {
            self.offset += token.len();
            true
        } else {
            false
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(character) = self.source[self.offset..].chars().next() {
            if !character.is_whitespace() {
                break;
            }
            self.offset += character.len_utf8();
        }
    }
}

fn parse_integer(value: &str) -> Result<i64> {
    let value = value.trim_end_matches(['u', 'U', 'l', 'L']);
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        i64::from_str_radix(hex, 16).map_err(Into::into)
    } else {
        value.parse::<i64>().map_err(Into::into)
    }
}

fn bool_value(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("hlsl-preprocessor-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn expands_nested_includes_and_selects_conditional_branches() {
        let root = temp_dir("nested");
        fs::write(
            root.join("lights.hlsl"),
            "float3 GetLight() { return 1; }\n",
        )
        .unwrap();
        fs::write(
            root.join("phong.hlsl"),
            "#include \"lights.hlsl\"\n            #if defined(ENABLE_PHONG) && LEVEL >= 2\nfloat3 Shade() { return GetLight(); }\n#else\nfloat3 Shade() { return 0; }\n#endif\n",
        )
        .unwrap();
        let source = "#define ENABLE_PHONG 1\n#define LEVEL 2\n#include \"phong.hlsl\"\n";
        let expanded = expand_source(source, &root.join("main.shader")).unwrap();
        assert!(expanded.contains("GetLight"));
        assert!(expanded.contains("return GetLight()"));
        assert!(!expanded.contains("return 0;"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn supports_else_undef_and_include_guards() {
        let root = temp_dir("guards");
        fs::write(
            root.join("shared.hlsl"),
            "#ifndef SHARED_HLSL\n#define SHARED_HLSL\nfloat Shared;\n#endif\n",
        )
        .unwrap();
        let source = "#define ENABLED 1\n#include \"shared.hlsl\"\n#include \"shared.hlsl\"\n#undef ENABLED\n#ifdef ENABLED\nwrong\n#else\nright\n#endif\n";
        let expanded = expand_source(source, &root.join("main.shader")).unwrap();
        assert_eq!(expanded.matches("float Shared").count(), 1);
        assert!(expanded.contains("right"));
        assert!(!expanded.contains("wrong"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ignores_directives_inside_block_and_line_comments() {
        let source = "/*\n#include \"missing.hlsl\"\n*/\n// #include \"also-missing.hlsl\"\n#if 1\nfloat Included;\n#endif\n";
        let expanded = expand_source(source, Path::new("main.shader")).unwrap();
        assert!(expanded.contains("#include \"missing.hlsl\""));
        assert!(expanded.contains("float Included;"));
    }

    #[test]
    fn reports_include_cycles_and_unbalanced_conditions() {
        let root = temp_dir("errors");
        fs::write(root.join("a.hlsl"), "#include \"b.hlsl\"\n").unwrap();
        fs::write(root.join("b.hlsl"), "#include \"a.hlsl\"\n").unwrap();
        assert!(
            format!("{:#}", expand_file(&root.join("a.hlsl")).unwrap_err())
                .contains("cyclic HLSL include")
        );
        assert!(expand_source("#if 1\ntext\n", &root.join("main.shader")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
