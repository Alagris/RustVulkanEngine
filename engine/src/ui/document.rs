use super::elem::{
    Button, Canvas, Element, Grid, Image, Label, List, Overlay, ScrollPane, Stack, Table,
};
use crate::assets::asset_ref::AssetSource;
use crate::assets::{AssetReference, AssetReferences};
use anyhow::{Context, Result, anyhow, bail};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) type ReferenceResolver<'a> = dyn Fn(&AssetSource) -> Result<AssetReference> + 'a;

/// Stable identity within one document. Removed identities are never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElementId(pub(crate) usize);

#[derive(Debug)]
pub(crate) struct Node {
    pub(crate) element: Box<dyn Element>,
    pub(crate) parent: Option<ElementId>,
    pub(crate) children: Vec<ElementId>,
    pub(crate) identity: u8,
    pub(crate) references: AssetReferences,
}

/// An owned tree of UI elements, with indexes kept in sync with edits.
#[derive(Debug)]
pub struct Document {
    pub(crate) nodes: Vec<Option<Box<Node>>>,
    root: ElementId,
    ids: HashMap<String, HashSet<ElementId>>,
    names: HashMap<String, HashSet<ElementId>>,
    classes: HashMap<String, HashSet<ElementId>>,
}

impl Document {
    pub fn new(root: impl Element + 'static) -> Result<Self> {
        Self::from_box(Box::new(root))
    }

    fn from_box(root: Box<dyn Element>) -> Result<Self> {
        let mut document = Self {
            nodes: vec![Some(Box::new(Node {
                element: root,
                parent: None,
                children: Vec::new(),
                identity: 0,
                references: Default::default(),
            }))],
            root: ElementId(0),
            ids: HashMap::new(),
            names: HashMap::new(),
            classes: HashMap::new(),
        };
        document.index(ElementId(0));
        Ok(document)
    }

    pub fn root(&self) -> ElementId {
        self.root
    }

    pub fn element(&self, id: ElementId) -> Option<&dyn Element> {
        Some(self.node(id)?.element.as_ref())
    }

    pub fn element_as<T: Element>(&self, id: ElementId) -> Option<&T> {
        (self.element(id)? as &dyn std::any::Any).downcast_ref()
    }

    /// Edit a typed element while keeping the document's indexes synchronized.
    pub fn edit<T: Element>(&mut self, id: ElementId, edit: impl FnOnce(&mut T)) -> Result<()> {
        if self.element_as::<T>(id).is_none() {
            bail!("element does not exist or has a different type");
        }
        self.unindex(id)?;
        let element = self.nodes[id.0]
            .as_mut()
            .expect("validated element")
            .element
            .as_mut();
        let element = (element as &mut dyn std::any::Any)
            .downcast_mut::<T>()
            .expect("validated type");
        edit(element);
        self.index(id);
        Ok(())
    }

    pub fn parent(&self, id: ElementId) -> Option<ElementId> {
        self.node(id)?.parent
    }

    pub fn children(&self, id: ElementId) -> Option<&[ElementId]> {
        Some(&self.node(id)?.children)
    }

    pub fn references(&self, id: ElementId) -> Option<&AssetReferences> {
        Some(&self.node(id)?.references)
    }

    pub fn append(
        &mut self,
        parent: ElementId,
        element: impl Element + 'static,
    ) -> Result<ElementId> {
        self.append_box(parent, Box::new(element))
    }

    fn append_box(&mut self, parent: ElementId, element: Box<dyn Element>) -> Result<ElementId> {
        let parent_node = self
            .node(parent)
            .ok_or_else(|| anyhow!("parent element does not exist"))?;
        if !parent_node.element.is_layout() {
            bail!(
                "element '{}' cannot contain children",
                parent_node.element.tag()
            );
        }
        let id = ElementId(self.nodes.len());
        self.nodes.push(Some(Box::new(Node {
            element,
            parent: Some(parent),
            children: Vec::new(),
            identity: 0,
            references: Default::default(),
        })));
        self.nodes[parent.0]
            .as_mut()
            .expect("validated parent")
            .children
            .push(id);
        self.index(id);
        Ok(id)
    }

    pub fn set_attribute(&mut self, id: ElementId, name: &str, value: &str) -> Result<()> {
        if name.is_empty() {
            bail!("attribute name must not be empty");
        }
        self.unindex(id)?;
        let result = self.nodes[id.0]
            .as_mut()
            .expect("validated element")
            .element
            .set_attribute(name, Some(value));
        self.index(id);
        result
    }

    pub fn remove_attribute(&mut self, id: ElementId, name: &str) -> Result<()> {
        self.unindex(id)?;
        let result = self.nodes[id.0]
            .as_mut()
            .expect("validated element")
            .element
            .set_attribute(name, None);
        self.index(id);
        result
    }

    pub fn set_text(&mut self, id: ElementId, text: impl Into<String>) -> Result<()> {
        self.nodes
            .get_mut(id.0)
            .and_then(Option::as_mut)
            .ok_or_else(|| anyhow!("element does not exist"))?
            .element
            .set_text(text.into())
    }

    pub fn remove(&mut self, id: ElementId) -> Result<()> {
        let node = self
            .node(id)
            .ok_or_else(|| anyhow!("element does not exist"))?;
        let parent = node
            .parent
            .ok_or_else(|| anyhow!("cannot remove the document root"))?;
        let mut pending = vec![id];
        let mut subtree = Vec::new();
        while let Some(current) = pending.pop() {
            pending.extend(
                self.node(current)
                    .expect("live subtree")
                    .children
                    .iter()
                    .copied(),
            );
            subtree.push(current);
        }
        self.nodes[parent.0]
            .as_mut()
            .expect("live parent")
            .children
            .retain(|child| *child != id);
        for current in subtree {
            self.unindex(current)?;
            self.nodes[current.0] = None;
        }
        Ok(())
    }

    /// Move an element to a position in another container, or reorder it in place.
    pub fn move_to(&mut self, id: ElementId, parent: ElementId, index: usize) -> Result<()> {
        let old_parent = self
            .node(id)
            .ok_or_else(|| anyhow!("element does not exist"))?
            .parent
            .ok_or_else(|| anyhow!("cannot move the document root"))?;
        let target = self
            .node(parent)
            .ok_or_else(|| anyhow!("parent element does not exist"))?;
        if !target.element.is_layout() {
            bail!("element '{}' cannot contain children", target.element.tag());
        }
        let final_length = target.children.len() - usize::from(parent == old_parent);
        if index > final_length {
            bail!("child position {index} is out of bounds");
        }
        let mut ancestor = Some(parent);
        while let Some(current) = ancestor {
            if current == id {
                bail!("cannot move an element into its own subtree");
            }
            ancestor = self.node(current).expect("live ancestor").parent;
        }
        self.nodes[old_parent.0]
            .as_mut()
            .expect("live parent")
            .children
            .retain(|child| *child != id);
        self.nodes[parent.0]
            .as_mut()
            .expect("live parent")
            .children
            .insert(index, id);
        self.nodes[id.0].as_mut().expect("live element").parent = Some(parent);
        Ok(())
    }

    pub fn find_id(&self, id: &str) -> Vec<ElementId> {
        lookup(&self.ids, id)
    }
    pub fn find_name(&self, name: &str) -> Vec<ElementId> {
        lookup(&self.names, name)
    }
    pub fn find_class(&self, class: &str) -> Vec<ElementId> {
        lookup(&self.classes, class)
    }

    pub fn parse(source: &str) -> Result<Self> {
        let spec = parse_spec(source, None, &mut Vec::new(), &resolve_reference)?;
        Self::from_spec(spec)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let spec = load_spec(path.as_ref(), &mut Vec::new(), &resolve_reference)?;
        Self::from_spec(spec)
    }

    pub(crate) fn from_spec(spec: Spec) -> Result<Self> {
        let references = spec.references.clone();
        let (root, children) = spec.into_element()?;
        let mut document = Self::from_box(root)?;
        document.nodes[0].as_mut().expect("root exists").references = references;
        fn append_all(
            document: &mut Document,
            parent: ElementId,
            children: Vec<Spec>,
        ) -> Result<()> {
            for child in children {
                let references = child.references.clone();
                let (element, descendants) = child.into_element()?;
                let id = document.append_box(parent, element)?;
                document.nodes[id.0]
                    .as_mut()
                    .expect("appended node exists")
                    .references = references;
                append_all(document, id, descendants)?;
            }
            Ok(())
        }
        append_all(&mut document, ElementId(0), children)?;
        Ok(document)
    }

    pub(crate) fn node(&self, id: ElementId) -> Option<&Node> {
        self.nodes.get(id.0)?.as_deref()
    }

    fn index(&mut self, id: ElementId) {
        let node = self.node(id).expect("indexing live node");
        let data = node.element.data();
        let id_value = data.attribute("id").map(|value| value.into_owned());
        let name = data.attribute("name").map(|value| value.into_owned());
        let classes = class_names(data.attribute("class").as_deref().unwrap_or(""))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if let Some(value) = id_value {
            self.ids.entry(value).or_default().insert(id);
        }
        if let Some(value) = name {
            self.names.entry(value).or_default().insert(id);
        }
        for class in classes {
            self.classes.entry(class).or_default().insert(id);
        }
    }

    fn unindex(&mut self, id: ElementId) -> Result<()> {
        let node = self
            .node(id)
            .ok_or_else(|| anyhow!("element does not exist"))?;
        let data = node.element.data();
        let id_value = data.attribute("id").map(|value| value.into_owned());
        let name = data.attribute("name").map(|value| value.into_owned());
        let classes = class_names(data.attribute("class").as_deref().unwrap_or(""))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if let Some(value) = id_value {
            remove_index(&mut self.ids, &value, id);
        }
        if let Some(value) = name {
            remove_index(&mut self.names, &value, id);
        }
        for class in classes {
            remove_index(&mut self.classes, &class, id);
        }
        Ok(())
    }
}

pub(crate) fn class_names(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(selectors::attr::SELECTOR_WHITESPACE)
        .filter(|name| !name.is_empty())
}

fn lookup(index: &HashMap<String, HashSet<ElementId>>, key: &str) -> Vec<ElementId> {
    let mut result = index
        .get(key)
        .map(|ids| ids.iter().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    result.sort_unstable();
    result
}

fn remove_index(index: &mut HashMap<String, HashSet<ElementId>>, key: &str, id: ElementId) {
    if let Some(ids) = index.get_mut(key) {
        ids.remove(&id);
        if ids.is_empty() {
            index.remove(key);
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Spec {
    tag: String,
    attributes: BTreeMap<String, String>,
    text: String,
    children: Vec<Spec>,
    references: AssetReferences,
}

impl Spec {
    pub(crate) fn to_xml(&self) -> String {
        fn escape(value: &str) -> String {
            value
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
                .replace('\r', "&#13;")
                .replace('\n', "&#10;")
                .replace('\t', "&#9;")
        }
        let mut output = format!("<{}", self.tag);
        for (name, value) in &self.attributes {
            output.push_str(&format!(" {name}=\"{}\"", escape(value)));
        }
        output.push('>');
        output.push_str(&escape(&self.text));
        for (key, reference) in &self.references {
            let kind = match reference.kind() {
                crate::assets::AssetKind::Prefab => "prefab",
                crate::assets::AssetKind::Ui => "ui",
                crate::assets::AssetKind::Data => "data",
            };
            output.push_str(&format!(
                "<Reference key=\"{}\" type=\"{kind}\" path=\"{}\"/>",
                escape(key),
                escape(reference.path())
            ));
        }
        for child in &self.children {
            output.push_str(&child.to_xml());
        }
        output.push_str(&format!("</{}>", self.tag));
        output
    }

    fn into_element(self) -> Result<(Box<dyn Element>, Vec<Spec>)> {
        let mut attributes = self.attributes;
        if matches!(self.tag.as_str(), "Label" | "Button") {
            attributes.insert("$text".to_owned(), self.text);
        } else if !self.text.trim().is_empty() {
            bail!("element '{}' does not support text", self.tag);
        }
        let value = serde_yaml::to_value(attributes)?;
        macro_rules! deserialize {
            ($ty:ty) => {
                Box::new(
                    serde_yaml::from_value::<$ty>(value).map_err(|error| {
                        anyhow!("invalid attributes for '{}': {error}", self.tag)
                    })?,
                )
            };
        }
        let element: Box<dyn Element> = match self.tag.as_str() {
            "Label" => deserialize!(Label),
            "Button" => deserialize!(Button),
            "Image" => deserialize!(Image),
            "Canvas" => deserialize!(Canvas),
            "ScrollPane" => deserialize!(ScrollPane),
            "Grid" => deserialize!(Grid),
            "List" => deserialize!(List),
            "Table" => deserialize!(Table),
            "Stack" => deserialize!(Stack),
            "Overlay" => deserialize!(Overlay),
            tag => bail!("unknown UI element '{tag}'"),
        };
        Ok((element, self.children))
    }
}

pub(crate) fn load_spec(
    path: &Path,
    stack: &mut Vec<PathBuf>,
    resolver: &ReferenceResolver<'_>,
) -> Result<Spec> {
    let path = path
        .canonicalize()
        .with_context(|| format!("failed to resolve UI document {}", path.display()))?;
    if let Some(context) = crate::assets::asset_ref::active_context()
        && !path.starts_with(&context.root)
    {
        bail!(
            "UI document {} is outside the asset directory {}",
            path.display(),
            context.root.display()
        );
    }
    if stack.contains(&path) {
        let mut chain = stack
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>();
        chain.push(path.display().to_string());
        bail!("cyclic UI document include: {}", chain.join(" -> "));
    }
    stack.push(path.clone());
    let result = (|| {
        let source = fs::read_to_string(&path)
            .with_context(|| format!("failed to read UI document {}", path.display()))?;
        parse_spec(&source, path.parent(), stack, resolver)
            .with_context(|| format!("invalid UI document {}", path.display()))
    })();
    stack.pop();
    result
}

fn parse_spec(
    source: &str,
    base: Option<&Path>,
    stack: &mut Vec<PathBuf>,
    resolver: &ReferenceResolver<'_>,
) -> Result<Spec> {
    let xml = roxmltree::Document::parse(source).context("failed to parse UI XML")?;
    fn add_attribute(
        attributes: &mut BTreeMap<String, String>,
        attribute: roxmltree::Attribute<'_, '_>,
    ) -> Result<()> {
        if let Some(namespace) = attribute.namespace() {
            if namespace == "http://www.w3.org/XML/1998/namespace" && attribute.name() == "lang" {
                attributes.insert("lang".to_owned(), attribute.value().to_owned());
                return Ok(());
            }
            bail!("XML namespaces are not supported for UI attributes");
        }
        attributes.insert(attribute.name().to_owned(), attribute.value().to_owned());
        Ok(())
    }
    fn resolve_template(
        tag: &str,
        attributes: &mut BTreeMap<String, String>,
        resolver: &ReferenceResolver<'_>,
    ) -> Result<()> {
        if matches!(tag, "List" | "Table")
            && let Some(template) = attributes.get("template")
        {
            let path = crate::assets::AssetKind::from_path(Path::new(template)).is_some()
                || template.contains(['/', '\\']);
            let source = AssetSource {
                kind: crate::assets::AssetKind::Ui,
                path: path.then(|| template.clone()),
                name: (!path).then(|| template.clone()),
            };
            let reference = resolver(&source)?;
            attributes.insert("template".into(), reference.path().to_owned());
        }
        Ok(())
    }
    fn parse_node(
        node: roxmltree::Node<'_, '_>,
        base: Option<&Path>,
        stack: &mut Vec<PathBuf>,
        resolver: &ReferenceResolver<'_>,
    ) -> Result<Spec> {
        if node.tag_name().namespace().is_some() {
            bail!("XML namespaces are not supported for UI elements");
        }
        if node.tag_name().name() == "Include" {
            let src = node
                .attribute("src")
                .ok_or_else(|| anyhow!("Include requires a src attribute"))?;
            if src.is_empty() {
                bail!("Include src must not be empty");
            }
            if node.children().any(|child| {
                child.is_element()
                    || child.is_text() && !child.text().unwrap_or("").trim().is_empty()
            }) {
                bail!("Include cannot contain children");
            }
            let base = base.ok_or_else(|| anyhow!("Include requires a document path"))?;
            let mut spec = load_spec(&base.join(src), stack, resolver)?;
            for attribute in node
                .attributes()
                .filter(|attribute| attribute.namespace().is_some() || attribute.name() != "src")
            {
                add_attribute(&mut spec.attributes, attribute)?;
            }
            resolve_template(&spec.tag, &mut spec.attributes, resolver)?;
            return Ok(spec);
        }
        let mut attributes = BTreeMap::new();
        for attribute in node.attributes() {
            add_attribute(&mut attributes, attribute)?;
        }
        resolve_template(node.tag_name().name(), &mut attributes, resolver)?;
        let mut children = Vec::new();
        let mut references = AssetReferences::new();
        let mut text = String::new();
        for child in node.children() {
            if child.is_element() {
                if child.tag_name().name() == "Reference" {
                    if child.tag_name().namespace().is_some() {
                        bail!("XML namespaces are not supported for UI elements");
                    }
                    if child.children().any(|node| {
                        node.is_element()
                            || node.is_text() && !node.text().unwrap_or("").trim().is_empty()
                    }) {
                        bail!("Reference cannot contain children");
                    }
                    let mut attributes = BTreeMap::new();
                    for attribute in child.attributes() {
                        add_attribute(&mut attributes, attribute)?;
                    }
                    let key = attributes
                        .remove("key")
                        .filter(|key| !key.trim().is_empty())
                        .ok_or_else(|| anyhow!("Reference requires a non-empty key"))?;
                    let source: AssetSource =
                        serde_yaml::from_value(serde_yaml::to_value(attributes)?)?;
                    if references.insert(key.clone(), resolver(&source)?).is_some() {
                        bail!("duplicate UI reference key '{key}'");
                    }
                } else {
                    children.push(parse_node(child, base, stack, resolver)?);
                }
            } else if child.is_text() {
                text.push_str(child.text().unwrap_or(""));
            }
        }
        Ok(Spec {
            tag: node.tag_name().name().to_owned(),
            attributes,
            text,
            children,
            references,
        })
    }
    parse_node(xml.root_element(), base, stack, resolver)
}

fn resolve_reference(source: &AssetSource) -> Result<AssetReference> {
    crate::assets::asset_ref::current_context()?.resolve(source)
}
