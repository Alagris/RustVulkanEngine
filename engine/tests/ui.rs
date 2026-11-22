use engine::ui::css::{Background, Color, Declaration, Length, Style};
use engine::ui::{
    Button, Canvas, Document, ElementData, Label, PSEUDO_STATES, Stack, StackOrientation,
    StyledDocument, Stylesheet,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Deserialize)]
struct Fixture {
    sheets: Vec<String>,
    scenarios: Vec<Scenario>,
}

#[derive(Deserialize)]
struct Scenario {
    #[serde(default)]
    states: BTreeMap<String, Vec<String>>,
    styles: BTreeMap<String, Style>,
}

#[test]
fn evaluated_document_fixtures() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("resources")
        .join("ui");
    let mut cases = fs::read_dir(&root)
        .expect("UI fixture directory must exist")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    cases.sort();
    assert_eq!(cases.len(), 8, "unexpected UI fixture count");
    let mut scenario_count = 0;
    for case in cases {
        let expected: Fixture =
            serde_yaml::from_str(&fs::read_to_string(case.join("expected.yaml")).unwrap()).unwrap();
        let document = Document::load(case.join("document.xml"))
            .unwrap_or_else(|error| panic!("{}: {error:#}", case.display()));
        let sheets: Vec<_> = expected
            .sheets
            .iter()
            .map(|name| Stylesheet::load(case.join(name)).unwrap())
            .collect();
        for scenario in expected.scenarios {
            scenario_count += 1;
            let mut styled = StyledDocument::new(
                Document::load(case.join("document.xml")).unwrap(),
                sheets.clone(),
            );
            for (id, states) in &scenario.states {
                let element = styled.document().find_id(id);
                assert_eq!(
                    element.len(),
                    1,
                    "{}: missing or duplicate id {id}",
                    case.display()
                );
                for state in states {
                    styled.set_state(element[0], state, true).unwrap();
                }
            }
            let mut actual = BTreeMap::new();
            fn walk(
                styled: &mut StyledDocument,
                id: engine::ui::ElementId,
                actual: &mut BTreeMap<String, Style>,
            ) {
                let node = styled.document().element(id).unwrap();
                let name = node
                    .data()
                    .attribute("id")
                    .expect("fixture elements must have ids")
                    .into_owned();
                let children = styled.document().children(id).unwrap().to_vec();
                assert!(
                    actual.insert(name, styled.style(id).unwrap()).is_none(),
                    "fixture ids must be unique"
                );
                for child in children {
                    walk(styled, child, actual);
                }
            }
            walk(&mut styled, document.root(), &mut actual);
            assert_eq!(actual, scenario.styles, "{}", case.display());
        }
    }
    assert_eq!(scenario_count, 9, "unexpected UI scenario count");
}

#[test]
fn document_edits_keep_indexes_consistent() {
    let mut document = Document::new(Canvas::default()).unwrap();
    let data = ElementData {
        id: Some("old".into()),
        name: Some("title".into()),
        class: Some("one two".into()),
        ..Default::default()
    };
    let label = document
        .append(
            document.root(),
            Label {
                data,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(document.find_id("old"), [label]);
    assert_eq!(document.find_name("title"), [label]);
    assert_eq!(document.find_class("two"), [label]);
    document.set_attribute(label, "id", "new").unwrap();
    document.set_attribute(label, "class", "three").unwrap();
    assert!(document.find_id("old").is_empty());
    assert!(document.find_class("one").is_empty());
    assert_eq!(document.find_id("new"), [label]);
    assert_eq!(document.find_class("three"), [label]);
    assert!(
        document
            .append(label, Button::default())
            .unwrap_err()
            .to_string()
            .contains("cannot contain")
    );
    document.remove(label).unwrap();
    assert!(document.find_id("new").is_empty());
    assert!(document.find_name("title").is_empty());
    assert!(document.find_class("three").is_empty());
    assert!(document.element(label).is_none());
}

#[test]
fn stack_orientation_is_explicitly_configurable() {
    let mut stack = Stack::default();
    assert_eq!(stack.orientation(), StackOrientation::Vertical);
    stack.set_orientation(StackOrientation::Horizontal);
    assert_eq!(stack.orientation(), StackOrientation::Horizontal);
    assert!(
        format!(
            "{:#}",
            Document::parse("<Stack orientation='diagonal'/>").unwrap_err()
        )
        .contains("orientation")
    );
    let mut document = Document::new(stack).unwrap();
    assert!(
        document
            .set_attribute(document.root(), "orientation", "diagonal")
            .unwrap_err()
            .to_string()
            .contains("orientation")
    );
    assert_eq!(
        document
            .element(document.root())
            .unwrap()
            .attribute("orientation")
            .as_deref(),
        Some("horizontal")
    );
}

#[test]
fn relational_selectors_follow_dynamic_state_and_document_edits() {
    let document =
        Document::parse(r#"<Canvas id="root"><Button id="one"/><Label id="two"/></Canvas>"#)
            .unwrap();
    let root = document.root();
    let one = document.find_id("one")[0];
    let two = document.find_id("two")[0];
    let sheet = Stylesheet::parse("Canvas:has(> Button:hover) { background: red; } Button:hover + Label { foreground: blue; } Canvas:has(> Button:focus) { font-size: 15px; }").unwrap();
    let mut styled = StyledDocument::new(document, vec![sheet]);
    assert!(styled.style(root).unwrap().is_empty());
    styled.set_state(one, "hover", true).unwrap();
    assert_eq!(
        styled.style(root).unwrap().background,
        Some(Background::Color(color("red")))
    );
    assert_eq!(styled.style(two).unwrap().foreground, Some(color("blue")));
    styled.set_state(one, "focus", true).unwrap();
    assert_eq!(
        styled.style(root).unwrap().font_size,
        Some(Length::Pixels(15.0))
    );
    styled.document_mut().remove(one).unwrap();
    assert!(styled.style(root).unwrap().is_empty());
    assert!(styled.style(two).unwrap().is_empty());
}

#[test]
fn stylesheet_tokens_handle_comments_and_urls() {
    let document = Document::parse("<Image/>").unwrap();
    let id = document.root();
    let sheet = Stylesheet::parse(
        r#"Image { background: url(foo[bar.png); foreground: /* comment */ red; }"#,
    )
    .unwrap();
    let mut styled = StyledDocument::new(document, vec![sheet]);
    let style = styled.style(id).unwrap();
    assert_eq!(
        style.background,
        Some(Background::Image("foo[bar.png".into()))
    );
    assert_eq!(style.foreground, Some(color("red")));
}

#[test]
fn moving_elements_rechecks_structural_selectors_and_rejects_cycles() {
    let document = Document::parse(r#"<Canvas id="root"><Stack id="left"><Button id="button"/></Stack><Stack id="right"/></Canvas>"#).unwrap();
    let left = document.find_id("left")[0];
    let right = document.find_id("right")[0];
    let button = document.find_id("button")[0];
    let sheet = Stylesheet::parse("#right > Button:first-child { foreground: green; }").unwrap();
    let mut styled = StyledDocument::new(document, vec![sheet]);
    assert!(styled.style(button).unwrap().is_empty());
    styled.document_mut().move_to(button, right, 0).unwrap();
    assert_eq!(
        styled.style(button).unwrap().foreground,
        Some(color("green"))
    );
    assert_eq!(styled.document().parent(button), Some(right));
    assert!(styled.document_mut().move_to(right, button, 0).is_err());
    assert!(styled.document_mut().move_to(right, right, 0).is_err());
    assert!(styled.document_mut().move_to(button, left, 2).is_err());
    assert_eq!(styled.document().parent(button), Some(right));
}

#[test]
fn replacing_stylesheets_preserves_cascade_and_sparse_state_overrides() {
    let document = Document::parse(
        r#"<Canvas id="root"><Button id="one" class="x"/><Button id="two"/></Canvas>"#,
    )
    .unwrap();
    let one = document.find_id("one")[0];
    let two = document.find_id("two")[0];
    let sheet = Stylesheet::parse(
        "Button { foreground: white; background: black; } .x:hover { foreground: red; }",
    )
    .unwrap();
    let mut styled = StyledDocument::new(document, vec![sheet]);
    styled.set_state(one, "hover", true).unwrap();
    assert_eq!(styled.override_count(one).unwrap(), 1);
    assert_eq!(styled.style(one).unwrap().foreground, Some(color("red")));
    let replacement = Stylesheet::parse(
        "Button { foreground: white; background: black; } .x:hover { foreground: blue; }",
    )
    .unwrap();
    assert_eq!(styled.set_stylesheets(vec![replacement]), [one]);
    assert_eq!(styled.style(one).unwrap().foreground, Some(color("blue")));
    assert_eq!(styled.style(two).unwrap().foreground, Some(color("white")));
    styled.set_state(one, "hover", false).unwrap();
    assert_eq!(styled.override_count(one).unwrap(), 0);
    assert_eq!(styled.style(one).unwrap().foreground, Some(color("white")));
    let updated = Stylesheet::parse(
        "Button { foreground: white; background: black; } .x:hover { foreground: green; }",
    )
    .unwrap();
    assert!(styled.set_stylesheets(vec![updated]).is_empty());
    styled.set_state(one, "hover", true).unwrap();
    assert_eq!(styled.style(one).unwrap().foreground, Some(color("green")));
    let base = Stylesheet::parse("Button { foreground: white; background: black; }").unwrap();
    assert_eq!(styled.set_stylesheets(vec![base]), [one]);
    assert_eq!(styled.style(one).unwrap().foreground, Some(color("white")));
}

#[test]
fn invalid_documents_and_stylesheets_report_errors() {
    for source in [
        "<Canvas>",
        "<Unknown/>",
        "<Label><Button/></Label>",
        "<Include src='other.xml'/>",
        "<Label dir='auto'/>",
    ] {
        assert!(Document::parse(source).is_err(), "{source}");
    }
    for source in [
        "Button { left: banana; }",
        "Button { right: 12em; }",
        "Button { left: NaNpx; }",
        "Button { left: 12 px; }",
        "Button { foreground: ; }",
        "Button > { foreground: red; }",
        "Button { foreground: red;",
        "Button { foreground: 'red; }",
        "/* unterminated",
        "Button:unknown { foreground: red; }",
        "Button:unknown-function(foo) { foreground: red; }",
        ":host { foreground: red; }",
        "Button:visited { foreground: red; }",
        ":is(Button:visited, Label) { foreground: red; }",
        ":where(Button:unknown, Label) { foreground: red; }",
        "Button::part(label) { foreground: red; }",
        "Button::unknown { foreground: red; }",
        "Button::highlight(name) { foreground: red; }",
        "Button::slotted(Label) { foreground: red; }",
        "Table:nth-col(2) { foreground: red; }",
    ] {
        assert!(Stylesheet::parse(source).is_err(), "{source}");
    }
}

#[test]
fn pseudo_states_are_sparse_and_can_be_combined() {
    assert_eq!(
        PSEUDO_STATES,
        ["hover", "active", "focus", "enabled", "disabled", "checked"]
    );
    for state in PSEUDO_STATES {
        let document = Document::parse(r#"<Button id="button"/>"#).unwrap();
        let button = document.root();
        let css = format!(
            "Button {{ foreground: white; background: black; }} Button:{state} {{ foreground: red; }}"
        );
        let mut styled = StyledDocument::new(document, vec![Stylesheet::parse(&css).unwrap()]);
        assert_eq!(styled.override_count(button).unwrap(), 0, "{state}");
        styled.set_state(button, state, true).unwrap();
        assert_eq!(
            styled.style(button).unwrap().foreground,
            Some(color("red")),
            "{state}"
        );
        assert_eq!(styled.override_count(button).unwrap(), 1, "{state}");
        styled.set_state(button, state, false).unwrap();
        assert_eq!(
            styled.style(button).unwrap().foreground,
            Some(color("white")),
            "{state}"
        );
        assert_eq!(styled.override_count(button).unwrap(), 0, "{state}");
    }
}

#[test]
fn unsupported_states_and_pseudo_elements_are_rejected() {
    let document = Document::parse("<Button/>").unwrap();
    let button = document.root();
    let mut styled = StyledDocument::new(document, vec![]);
    for state in [
        "hovre",
        "focus-visible",
        "focus-within",
        "target",
        "indeterminate",
    ] {
        assert!(
            styled
                .set_state(button, state, true)
                .unwrap_err()
                .to_string()
                .contains("unsupported pseudo-state")
        );
        assert!(Stylesheet::parse(&format!("Button:{state} {{ foreground: red; }}")).is_err());
    }
    for pseudo in ["before", "after", "marker", "placeholder"] {
        assert!(Stylesheet::parse(&format!("Button::{pseudo} {{ foreground: red; }}")).is_err());
    }
    assert!(Stylesheet::parse("Button:before { foreground: red; }").is_err());
}

fn color(value: &str) -> Color {
    value.parse().unwrap()
}

#[test]
fn typed_values_and_invalid_attributes_are_checked() {
    let mut doc = Document::parse("<Canvas id='root'><Stack id='stack' orientation='horizontal'><Image id='image' src='icon.png'/><Label id='label'>Hello</Label></Stack></Canvas>").unwrap();
    let stack = doc.find_id("stack")[0];
    let image = doc.find_id("image")[0];
    let label = doc.find_id("label")[0];
    assert_eq!(
        doc.element(stack)
            .unwrap()
            .attribute("orientation")
            .as_deref(),
        Some("horizontal")
    );
    assert_eq!(
        doc.element(image).unwrap().attribute("src").as_deref(),
        Some("icon.png")
    );
    assert_eq!(doc.element(label).unwrap().text(), Some("Hello"));
    assert!(doc.set_text(image, "bad").is_err());
    assert!(doc.set_attribute(label, "data-unknown", "bad").is_err());
    assert_eq!(doc.find_id("label"), [label]);
    assert!(
        format!(
            "{:#}",
            Document::parse("<Label data-unknown='bad'/>").unwrap_err()
        )
        .contains("data-unknown")
    );
    assert!(
        Document::parse("<Label id='ok' src='wrong'/>")
            .unwrap_err()
            .to_string()
            .contains("src")
    );
    assert!(
        Document::parse("<Stack dir='auto'/>")
            .unwrap_err()
            .to_string()
            .contains("direction")
    );
    assert!(
        format!("{:#}", Document::parse("<Image>bad</Image>").unwrap_err())
            .contains("does not support text")
    );
    let sheet = Stylesheet::parse(r#"Image[src="icon.png"] { left: -2.5px; top: 25%; foreground: #f008; background: url("a b.png"); }"#).unwrap();
    assert_eq!(
        sheet.rules()[0].selector_source(),
        r#"Image[src="icon.png"]"#
    );
    assert_eq!(
        sheet.rules()[0].declarations()[0],
        Declaration::Left(Length::Pixels(-2.5))
    );
    let mut styled = StyledDocument::new(doc, vec![sheet]);
    let style = styled.style(image).unwrap();
    assert_eq!(style.left, Some(Length::Pixels(-2.5)));
    assert_eq!(style.top, Some(Length::Percent(25.0)));
    assert_eq!(style.foreground, Some(color("#f008")));
    assert_eq!(style.background, Some(Background::Image("a b.png".into())));
    assert_eq!(
        serde_yaml::from_str::<Style>(&serde_yaml::to_string(&style).unwrap()).unwrap(),
        style
    );
    for (css, reason) in [
        ("* { made-up: red; }", "unsupported style property"),
        ("* { foreground: banana; }", "unknown color"),
        ("* { font-size: -1px; }", "non-negative"),
        ("* { text-align: middle; }", "left, center or right"),
        ("* { background: url(); }", "must not be empty"),
    ] {
        let error = Stylesheet::parse(css).unwrap_err();
        assert!(format!("{error:#}").contains(reason), "{css}: {error:#}");
    }
}

#[test]
fn typed_attributes_can_be_changed_and_removed() {
    let doc =
        Document::parse("<Stack orientation='horizontal'><Image src='old.png'/></Stack>").unwrap();
    let stack = doc.root();
    let image = doc.children(stack).unwrap()[0];
    let sheet = Stylesheet::parse(
        "Stack[orientation='vertical'] > Image[src='new.png'] { foreground: red; }",
    )
    .unwrap();
    let mut styled = StyledDocument::new(doc, vec![sheet]);
    styled
        .document_mut()
        .set_attribute(stack, "orientation", "vertical")
        .unwrap();
    styled
        .document_mut()
        .set_attribute(image, "src", "new.png")
        .unwrap();
    assert_eq!(styled.style(image).unwrap().foreground, Some(color("red")));
    assert!(
        styled
            .document_mut()
            .set_attribute(stack, "orientation", "bad")
            .is_err()
    );
    assert_eq!(styled.style(image).unwrap().foreground, Some(color("red")));
    styled
        .document_mut()
        .remove_attribute(stack, "orientation")
        .unwrap();
    assert_eq!(
        styled
            .document()
            .element_as::<Stack>(stack)
            .unwrap()
            .orientation(),
        StackOrientation::Vertical
    );
    assert!(styled.style(image).unwrap().is_empty());
}

#[test]
fn typed_edits_keep_indexes_and_selectors_current() {
    let doc = Document::parse("<Canvas><Label id='old' class='before'>old text</Label></Canvas>")
        .unwrap();
    let label = doc.find_id("old")[0];
    let sheet = Stylesheet::parse(".after { foreground: #ff0000; }").unwrap();
    let mut styled = StyledDocument::new(doc, vec![sheet]);
    styled
        .document_mut()
        .edit::<Label>(label, |element| {
            element.data.id = Some("new".into());
            element.data.class = Some("after".into());
            element.text = "new text".into();
        })
        .unwrap();
    assert!(styled.document().find_id("old").is_empty());
    assert!(styled.document().find_class("before").is_empty());
    assert_eq!(styled.document().find_id("new"), [label]);
    assert_eq!(styled.document().find_class("after"), [label]);
    assert_eq!(
        styled.document().element_as::<Label>(label).unwrap().text,
        "new text"
    );
    assert_eq!(styled.style(label).unwrap().foreground, Some(color("red")));
    assert!(styled.document_mut().edit::<Button>(label, |_| {}).is_err());
    assert_eq!(styled.document().find_id("new"), [label]);
}

#[test]
fn include_cycles_are_reported() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("resources")
        .join("ui_errors")
        .join("cycle.xml");
    let error = Document::load(path).unwrap_err();
    assert!(format!("{error:#}").contains("cyclic UI document include"));
}

#[test]
fn included_root_rejects_unknown_overrides() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("resources")
        .join("ui_errors")
        .join("invalid_include_attribute.xml");
    let error = Document::load(path).unwrap_err();
    assert!(format!("{error:#}").contains("data-unknown"));
}

#[test]
fn repeated_includes_are_independent_and_indexes_support_duplicate_ids() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("resources")
        .join("ui_errors")
        .join("repeated.xml");
    let mut document = Document::load(path).unwrap();
    let buttons = document.find_id("inside");
    assert_eq!(buttons.len(), 2);
    document
        .set_attribute(buttons[0], "class", "changed")
        .unwrap();
    assert_eq!(document.find_class("action"), [buttons[1]]);
    assert_eq!(document.find_class("changed"), [buttons[0]]);
}

#[test]
fn generated_custom_elements_support_renames_text_and_typed_styles() {
    use engine::ui::elem::{Attributes, Element, UiStyle, element};

    #[derive(Clone, Debug, Default, UiStyle)]
    struct BadgeStyle {
        foreground: Option<Color>,
    }

    #[element(style = BadgeStyle)]
    #[derive(Clone, Debug, Default, Deserialize, Element)]
    #[serde(deny_unknown_fields)]
    struct Badge {
        #[serde(rename = "asset-name")]
        asset: Option<String>,
        #[ui(text)]
        #[serde(default, rename = "$text")]
        text: String,
    }

    let mut badge: Badge = serde_yaml::from_str(
        "id: custom\nasset-name: icon\nstyle: 'foreground: red;'\n$text: badge",
    )
    .unwrap();
    assert_eq!(badge.tag(), "Badge");
    assert_eq!(badge.attribute("id").as_deref(), Some("custom"));
    assert_eq!(badge.attribute("asset-name").as_deref(), Some("icon"));
    assert_eq!(badge.attribute("asset"), None);
    assert_eq!(badge.attribute("$text"), None);
    assert_eq!(badge.text(), Some("badge"));
    assert_eq!(badge.data.style.foreground, Some(color("red")));
    assert!(!badge.is_layout());
    badge.set_attribute("asset-name", Some("updated")).unwrap();
    badge
        .set_attribute("style", Some("foreground: blue;"))
        .unwrap();
    assert_eq!(badge.asset.as_deref(), Some("updated"));
    assert_eq!(badge.data.style.foreground, Some(color("blue")));
    assert!(
        badge
            .set_attribute("style", Some("background: red;"))
            .unwrap_err()
            .to_string()
            .contains("BadgeStyle")
    );
    assert_eq!(badge.data.style.foreground, Some(color("blue")));
    badge.set_attribute("asset-name", None).unwrap();
    badge.set_text("updated text".into()).unwrap();
    assert!(badge.asset.is_none());
    assert_eq!(badge.text(), Some("updated text"));
    assert!(badge.set_attribute("unknown", None).is_err());
    let doc = Document::new(badge).unwrap();
    let id = doc.root();
    let mut styled = StyledDocument::new(doc, vec![]);
    assert_eq!(styled.style(id).unwrap().foreground, Some(color("blue")));

    let sheet = Stylesheet::parse("Badge { background: red; }").unwrap();
    assert_eq!(styled.set_stylesheets(vec![sheet]), [id]);
    assert!(
        styled
            .style(id)
            .unwrap_err()
            .to_string()
            .contains("BadgeStyle")
    );
}

#[test]
fn generated_layouts_and_attribute_groups_work_without_manual_implementations() {
    use engine::ui::elem::{Attributes, Element, Layout, element};

    #[element(layout)]
    #[derive(Debug, Default, Deserialize, Element)]
    #[serde(deny_unknown_fields)]
    struct Panel {
        #[serde(rename = "panel-title")]
        title: String,
    }

    fn assert_layout<T: Layout>(_: &T) {}
    let panel: Panel =
        serde_yaml::from_str("id: root\npanel-title: test\nstyle: 'left: 10px;'").unwrap();
    assert_layout(&panel);
    assert_eq!(panel.tag(), "Panel");
    assert_eq!(panel.attribute("panel-title").as_deref(), Some("test"));
    let mut doc = Document::new(panel).unwrap();
    let root = doc.root();
    let child = doc.append(root, Label::default()).unwrap();
    assert_eq!(doc.children(root).unwrap(), [child]);
    assert!(
        doc.remove_attribute(root, "panel-title")
            .unwrap_err()
            .to_string()
            .contains("required attribute")
    );
    assert_eq!(doc.element_as::<Panel>(root).unwrap().title, "test");
    assert_eq!(doc.find_id("root"), [root]);
    let mut styled = StyledDocument::new(doc, vec![]);
    assert_eq!(styled.style(root).unwrap().left, Some(Length::Pixels(10.0)));
}

#[test]
fn inline_styles_are_typed_and_override_stylesheets() {
    use engine::ui::elem::{Image, LabelStyle};
    let doc = Document::parse("<Canvas><Label id='text' style='foreground: red; font-size: 15px;'/><Image id='image' style='background: blue; left: 20%;'/></Canvas>").unwrap();
    let label = doc.find_id("text")[0];
    let image = doc.find_id("image")[0];
    let label_style: &LabelStyle = &doc.element_as::<Label>(label).unwrap().data.style;
    assert_eq!(label_style.foreground, Some(color("red")));
    assert_eq!(label_style.font_size, Some(Length::Pixels(15.0)));
    assert_eq!(
        doc.element_as::<Image>(image).unwrap().data.style.left,
        Some(Length::Percent(20.0))
    );
    let sheet = Stylesheet::parse(
        "Label { foreground: green; } Label:hover { foreground: blue; } [style] { top: 2px; }",
    )
    .unwrap();
    let mut styled = StyledDocument::new(doc, vec![sheet]);
    styled.set_state(label, "hover", true).unwrap();
    assert_eq!(styled.style(label).unwrap().foreground, Some(color("red")));
    assert_eq!(styled.style(image).unwrap().top, Some(Length::Pixels(2.0)));
    assert_eq!(styled.override_count(label).unwrap(), 0);
    styled
        .document_mut()
        .remove_attribute(label, "style")
        .unwrap();
    assert_eq!(styled.style(label).unwrap().foreground, Some(color("blue")));
    assert!(
        styled
            .document()
            .element_as::<Label>(label)
            .unwrap()
            .data
            .style
            .foreground
            .is_none()
    );
    styled
        .document_mut()
        .edit::<Label>(label, |label| {
            label.data.style.foreground = Some(color("yellow"))
        })
        .unwrap();
    assert_eq!(
        styled.style(label).unwrap().foreground,
        Some(color("yellow"))
    );
    assert_eq!(styled.override_count(label).unwrap(), 0);
}

#[test]
fn invalid_inline_styles_report_errors_without_changing_fields_or_indexes() {
    let mut doc = Document::parse("<Image id='image' style='background: blue;'/>").unwrap();
    let image = doc.root();
    for (style, reason) in [
        ("font-size: 12px;", "ImageStyle"),
        ("foreground: banana;", "unknown color"),
        ("left: 3em;", "pixel or percentage"),
        ("foreground: red; } Label { background: blue;", "inline"),
    ] {
        let source = format!("<Image style='{style}'/>");
        let error = Document::parse(&source).unwrap_err();
        assert!(format!("{error:#}").contains(reason), "{source}: {error:#}");
        let error = doc.set_attribute(image, "style", style).unwrap_err();
        assert!(format!("{error:#}").contains(reason), "{style}: {error:#}");
        assert_eq!(doc.find_id("image"), [image]);
        assert_eq!(
            doc.element_as::<engine::ui::Image>(image)
                .unwrap()
                .data
                .style
                .background,
            Some(Background::Color(color("blue")))
        );
    }
}

#[test]
fn inline_style_strings_round_trip_typed_values() {
    let style = Style::parse_inline(r#"background: url("a b.png"); foreground: #f008; left: -2px; right: 3.125%; top: 0px; bottom: 1%; font-size: 12px; text-align: center;"#).unwrap();
    assert_eq!(
        Style::parse_inline(&style.to_inline_string()).unwrap(),
        style
    );
    assert_eq!(
        Style::parse_inline("foreground: red; foreground: blue;")
            .unwrap()
            .foreground,
        Some(color("blue"))
    );
    assert_eq!(
        Style::parse_inline(" /* empty */ ").unwrap(),
        Style::default()
    );
    for source in [
        "foreground: red; }",
        "@import 'other';",
        "Label { foreground: red; }",
        "foreground: red; /* unclosed",
    ] {
        assert!(Style::parse_inline(source).is_err(), "{source}");
    }
}
