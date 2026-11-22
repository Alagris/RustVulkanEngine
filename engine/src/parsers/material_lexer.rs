#[derive(Clone, Debug)]
pub(crate) enum Token {
    Shader,
    Pass,
    Face,
    Queue,
    Hlsl,
    LBrace,
    RBrace,
    Identifier(String),
    Quoted(String),
    Text(String),
}

#[derive(Clone, Debug)]
pub(crate) struct LexError {
    pub(crate) offset: usize,
    pub(crate) message: String,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} at byte {}", self.message, self.offset)
    }
}

impl std::error::Error for LexError {}

impl std::fmt::Display for Token {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

pub(crate) struct ShaderLexer<'input> {
    source: &'input str,
    offset: usize,
    in_hlsl: bool,
    hlsl_depth: usize,
    after_hlsl_keyword: bool,
}

impl<'input> ShaderLexer<'input> {
    pub(crate) fn new(source: &'input str) -> Self {
        Self {
            source,
            offset: 0,
            in_hlsl: false,
            hlsl_depth: 0,
            after_hlsl_keyword: false,
        }
    }

    fn take_while(&mut self, predicate: impl Fn(char) -> bool) -> &'input str {
        let start = self.offset;
        while let Some(character) = self.source[self.offset..].chars().next() {
            if !predicate(character) {
                break;
            }
            self.offset += character.len_utf8();
        }
        &self.source[start..self.offset]
    }

    fn quoted(&mut self) -> std::result::Result<Token, LexError> {
        let start = self.offset;
        self.offset += 1;
        let mut escaped = false;
        while let Some(character) = self.source[self.offset..].chars().next() {
            self.offset += character.len_utf8();
            if escaped {
                if !matches!(character, '"' | '\\' | 'n' | 'r' | 't') {
                    return Err(LexError {
                        offset: self.offset - character.len_utf8() - 1,
                        message: format!("unsupported escape '\\{}'", character),
                    });
                }
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                return Ok(Token::Quoted(self.source[start..self.offset].to_owned()));
            }
        }
        Err(LexError {
            offset: start,
            message: "unterminated quoted string".to_owned(),
        })
    }

    fn comment(&mut self) -> &'input str {
        let start = self.offset;
        if self.source[self.offset..].starts_with("//") {
            self.offset += 2;
            self.take_while(|character| character != '\n');
        } else {
            self.offset += 2;
            while self.offset < self.source.len() && !self.source[self.offset..].starts_with("*/") {
                let character = self.source[self.offset..].chars().next().unwrap();
                self.offset += character.len_utf8();
            }
            if self.source[self.offset..].starts_with("*/") {
                self.offset += 2;
            }
        }
        &self.source[start..self.offset]
    }
}

impl Iterator for ShaderLexer<'_> {
    type Item = std::result::Result<(usize, Token, usize), LexError>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.offset < self.source.len() {
            let start = self.offset;
            let rest = &self.source[self.offset..];
            let character = rest.chars().next().unwrap();

            if rest.starts_with("//") || rest.starts_with("/*") {
                let text = self.comment().to_owned();
                if self.in_hlsl {
                    return Some(Ok((start, Token::Text(text), self.offset)));
                }
                continue;
            }

            if character.is_whitespace() {
                let text = self.take_while(char::is_whitespace).to_owned();
                if self.in_hlsl {
                    return Some(Ok((start, Token::Text(text), self.offset)));
                }
                continue;
            }

            let token = if character == '"' {
                match self.quoted() {
                    Ok(token) => token,
                    Err(error) => return Some(Err(error)),
                }
            } else if character == '\'' && self.in_hlsl {
                let start = self.offset;
                self.offset += 1;
                let mut escaped = false;
                let mut terminated = false;
                while let Some(character) = self.source[self.offset..].chars().next() {
                    self.offset += character.len_utf8();
                    if escaped {
                        escaped = false;
                    } else if character == '\\' {
                        escaped = true;
                    } else if character == '\'' {
                        terminated = true;
                        break;
                    }
                }
                if !terminated {
                    return Some(Err(LexError {
                        offset: start,
                        message: "unterminated character literal".to_owned(),
                    }));
                }
                Token::Text(self.source[start..self.offset].to_owned())
            } else if character.is_ascii_alphabetic() || character == '_' {
                let word = self
                    .take_while(|character| character.is_ascii_alphanumeric() || character == '_');
                match word {
                    "shader" => Token::Shader,
                    "pass" => Token::Pass,
                    "face" => Token::Face,
                    "queue" => Token::Queue,
                    "hlsl" => Token::Hlsl,
                    _ => Token::Identifier(word.to_owned()),
                }
            } else {
                self.offset += character.len_utf8();
                match character {
                    '{' => {
                        if self.after_hlsl_keyword {
                            self.in_hlsl = true;
                            self.hlsl_depth = 1;
                            self.after_hlsl_keyword = false;
                        } else if self.in_hlsl {
                            self.hlsl_depth += 1;
                        }
                        Token::LBrace
                    }
                    '}' => {
                        if self.in_hlsl {
                            self.hlsl_depth -= 1;
                            if self.hlsl_depth == 0 {
                                self.in_hlsl = false;
                            }
                        }
                        Token::RBrace
                    }
                    _ if self.in_hlsl => Token::Text(character.to_string()),
                    _ => {
                        return Some(Err(LexError {
                            offset: start,
                            message: format!("unexpected character '{}'", character),
                        }));
                    }
                }
            };

            self.after_hlsl_keyword = !self.in_hlsl && matches!(token, Token::Hlsl);
            let token = if self.in_hlsl {
                match token {
                    Token::Identifier(text) => Token::Text(text),
                    Token::Quoted(text) => Token::Text(text),
                    Token::Shader => Token::Text("shader".to_owned()),
                    Token::Pass => Token::Text("pass".to_owned()),
                    Token::Face => Token::Text("face".to_owned()),
                    Token::Queue => Token::Text("queue".to_owned()),
                    Token::Hlsl => Token::Text("hlsl".to_owned()),
                    token => token,
                }
            } else {
                token
            };
            return Some(Ok((start, token, self.offset)));
        }
        None
    }
}
