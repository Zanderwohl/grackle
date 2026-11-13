//! A lang file as `toml_edit` holds it, so an edit touches only the lines it changes: order,
//! comments, blank lines, dotted keys and `[headers]` all survive a save.

use std::collections::{BTreeSet, HashMap};

use toml_edit::{DocumentMut, Item, Key, Table, TableLike, Value};

pub type KeyPath = Vec<String>;

pub fn dotted(path: &[String]) -> String {
    path.join(".")
}

/// `get!` splits on `.`, so a segment cannot hold one; the rest keeps keys bare in the file.
pub fn valid_segment(segment: &str) -> bool {
    !segment.is_empty() && segment.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `None` if any segment is invalid or the text is empty.
pub fn parse_path(text: &str) -> Option<KeyPath> {
    let path: KeyPath = text.trim().split('.').map(str::to_string).collect();
    path.iter().all(|segment| valid_segment(segment)).then_some(path)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LangError {
    BadKey,
    Exists(String),
    /// A key on the way holds a value, so it cannot also be a table.
    NotATable(String),
    Missing(String),
}

impl LangError {
    pub fn message(&self) -> String {
        match self {
            LangError::BadKey => crate::get!("lang_editor.error.bad_key"),
            LangError::Exists(key) => crate::get!("lang_editor.error.exists", "key", key),
            LangError::NotATable(key) => crate::get!("lang_editor.error.not_a_table", "key", key),
            LangError::Missing(key) => crate::get!("lang_editor.error.missing", "key", key),
        }
    }
}

/// A leaf as shown: strings are edited, anything else is shown as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Leaf {
    Text(String),
    Other(String),
}

impl Leaf {
    pub fn as_text(&self) -> &str {
        match self {
            Leaf::Text(text) | Leaf::Other(text) => text,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LangDoc {
    doc: DocumentMut,
}

impl Default for LangDoc {
    fn default() -> Self {
        Self { doc: DocumentMut::new() }
    }
}

impl LangDoc {
    pub fn parse(text: &str) -> Result<Self, String> {
        text.parse::<DocumentMut>().map(|doc| Self { doc }).map_err(|e| e.to_string())
    }

    pub fn text(&self) -> String {
        self.doc.to_string()
    }

    /// Every leaf, in the order the file holds them.
    pub fn leaves(&self) -> Vec<(KeyPath, Leaf)> {
        let mut out = Vec::new();
        collect(self.doc.as_table(), &mut Vec::new(), &mut out);
        out
    }

    pub fn leaf(&self, path: &[String]) -> Option<Leaf> {
        match self.item(path)? {
            Item::Value(value) => Some(leaf_of(value)),
            _ => None,
        }
    }

    pub fn contains(&self, path: &[String]) -> bool {
        self.item(path).is_some()
    }

    fn item(&self, path: &[String]) -> Option<&Item> {
        let (last, parents) = path.split_last()?;
        let mut table: &dyn TableLike = self.doc.as_table();
        for segment in parents {
            table = table.get(segment)?.as_table_like()?;
        }
        table.get(last)
    }

    fn table_mut(&mut self, path: &[String]) -> Option<&mut dyn TableLike> {
        let mut table: &mut dyn TableLike = self.doc.as_table_mut();
        for segment in path {
            table = table.get_mut(segment)?.as_table_like_mut()?;
        }
        Some(table)
    }

    /// Replaces a leaf's text keeping its spacing and comment, or adds the key if it is new.
    pub fn set(&mut self, path: &[String], text: &str) -> Result<(), LangError> {
        let Some((last, parents)) = path.split_last() else { return Err(LangError::BadKey) };
        if let Some(Item::Value(value)) = self.table_mut(parents).and_then(|table| table.get_mut(last)) {
            let decor = value.decor().clone();
            let mut fresh = Value::from(text);
            *fresh.decor_mut() = decor;
            *value = fresh;
            return Ok(());
        }
        self.insert(path, text)
    }

    /// A new key at the end of its table. Tables on the way that do not exist are made: dotted
    /// under a dotted table, as `[headers]` otherwise, and a new header lands after its parent's
    /// last table (`toml_edit` places a table with no position after whatever precedes it).
    pub fn insert(&mut self, path: &[String], text: &str) -> Result<(), LangError> {
        if path.is_empty() || !path.iter().all(|segment| valid_segment(segment)) {
            return Err(LangError::BadKey);
        }
        let (last, parents) = path.split_last().unwrap();
        let mut table: &mut dyn TableLike = self.doc.as_table_mut();
        for (depth, segment) in parents.iter().enumerate() {
            let dotted_parent = table.is_dotted();
            if !table.contains_key(segment) {
                let mut fresh = Table::new();
                fresh.set_implicit(true);
                fresh.set_dotted(dotted_parent);
                table.insert(segment, Item::Table(fresh));
            }
            table = table
                .get_mut(segment)
                .and_then(Item::as_table_like_mut)
                .ok_or_else(|| LangError::NotATable(dotted(&path[..=depth])))?;
        }
        if table.contains_key(last) {
            return Err(LangError::Exists(dotted(path)));
        }
        table.insert(last, Item::Value(Value::from(text)));
        Ok(())
    }

    /// Renames the last segment of `path`, a leaf or a whole subtree, in place.
    pub fn rename(&mut self, path: &[String], to: &str) -> Result<(), LangError> {
        if !valid_segment(to) {
            return Err(LangError::BadKey);
        }
        let Some((last, parents)) = path.split_last() else { return Err(LangError::BadKey) };
        if last == to {
            return Ok(());
        }
        let table = self.table_mut(parents).ok_or_else(|| LangError::Missing(dotted(path)))?;
        if !table.contains_key(last) {
            return Err(LangError::Missing(dotted(path)));
        }
        if table.contains_key(to) {
            let mut taken = parents.to_vec();
            taken.push(to.to_string());
            return Err(LangError::Exists(dotted(&taken)));
        }
        // Everything from the renamed key on comes out and goes back in order, so the key keeps
        // its place: `TableLike` has no rename.
        let keys: Vec<Key> = table.iter().filter_map(|(name, _)| table.key(name).cloned()).collect();
        let from = keys.iter().position(|key| key.get() == last).unwrap();
        let moved: Vec<(Key, Item)> = keys[from..]
            .iter()
            .filter_map(|key| table.remove(key.get()).map(|item| (key.clone(), item)))
            .collect();
        for (key, item) in moved {
            let key = match key.get() == last {
                true => Key::new(to)
                    .with_leaf_decor(key.leaf_decor().clone())
                    .with_dotted_decor(key.dotted_decor().clone()),
                false => key,
            };
            table.entry_format(&key).or_insert(item);
        }
        Ok(())
    }

    /// Removes a leaf or subtree, and any table it leaves empty.
    pub fn remove(&mut self, path: &[String]) -> bool {
        let Some((last, parents)) = path.split_last() else { return false };
        let removed = self.table_mut(parents).and_then(|table| table.remove(last)).is_some();
        if removed {
            for depth in (1..=parents.len()).rev() {
                let empty = self.table_mut(&parents[..depth]).is_some_and(|table| table.is_empty());
                if !empty {
                    break;
                }
                if let Some(table) = self.table_mut(&parents[..depth - 1]) {
                    table.remove(&parents[depth - 1]);
                }
            }
        }
        removed
    }
}

fn leaf_of(value: &Value) -> Leaf {
    match value {
        Value::String(text) => Leaf::Text(text.value().clone()),
        other => Leaf::Other(other.clone().decorated("", "").to_string()),
    }
}

fn collect(table: &dyn TableLike, path: &mut KeyPath, out: &mut Vec<(KeyPath, Leaf)>) {
    for (key, item) in table.iter() {
        path.push(key.to_string());
        match item {
            Item::Value(Value::InlineTable(inline)) => collect(inline, path, out),
            Item::Value(value) => out.push((path.clone(), leaf_of(value))),
            Item::Table(child) => collect(child, path, out),
            Item::ArrayOfTables(_) | Item::None => {}
        }
        path.pop();
    }
}

/// What a value is checked against: every pack's `en-US`, merged as the loader merges it.
#[derive(Debug, Clone, Default)]
pub struct Reference {
    pub order: Vec<KeyPath>,
    values: HashMap<KeyPath, String>,
}

impl Reference {
    pub fn from_table(table: &toml::Table) -> Self {
        let mut reference = Reference::default();
        flatten(table, &mut Vec::new(), &mut reference);
        reference
    }

    pub fn get(&self, path: &[String]) -> Option<&str> {
        self.values.get(path).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

fn flatten(table: &toml::Table, path: &mut KeyPath, into: &mut Reference) {
    for (key, value) in table {
        path.push(key.clone());
        match value {
            toml::Value::Table(child) => flatten(child, path, into),
            toml::Value::String(text) => {
                into.order.push(path.clone());
                into.values.insert(path.clone(), text.clone());
            }
            other => {
                into.order.push(path.clone());
                into.values.insert(path.clone(), other.to_string());
            }
        }
        path.pop();
    }
}

/// `ensure_lang` writes this in front of a key it stubbed.
pub const STUB_PREFIX: &str = "MISSING ";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Marks {
    /// In the reference, not in the file.
    pub missing: bool,
    pub stub: bool,
    /// Its `{ name }` slots differ from the reference's.
    pub placeholders: bool,
    /// In the file, not in the reference.
    pub extra: bool,
}

impl Marks {
    pub fn any(self) -> bool {
        self.missing || self.stub || self.placeholders || self.extra
    }

    /// `reference` is `None` when the open file is the reference itself.
    pub fn of(value: Option<&str>, reference: Option<&Reference>, path: &[String]) -> Marks {
        let stub = value.is_some_and(|value| value.starts_with(STUB_PREFIX));
        let Some(reference) = reference else { return Marks { stub, ..Default::default() } };
        let wanted = reference.get(path);
        Marks {
            missing: value.is_none() && wanted.is_some(),
            stub,
            placeholders: match (value, wanted) {
                (Some(value), Some(wanted)) if !stub => placeholders(value) != placeholders(wanted),
                _ => false,
            },
            extra: value.is_some() && wanted.is_none(),
        }
    }
}

pub fn placeholders(text: &str) -> BTreeSet<String> {
    crate::common::lang::placeholders_in(text)
}

/// The keys of the open file and the reference as one tree: the file's in its order, then what
/// only the reference has, each under its own parent.
#[derive(Debug, Clone, Default)]
pub struct Node {
    pub name: String,
    pub path: KeyPath,
    pub leaf: bool,
    pub children: Vec<Node>,
}

impl Node {
    pub fn build(doc: &LangDoc, reference: Option<&Reference>) -> Node {
        let mut root = Node::default();
        for (path, _) in doc.leaves() {
            root.add(&path, 0);
        }
        if let Some(reference) = reference {
            for path in &reference.order {
                root.add(path, 0);
            }
        }
        root
    }

    fn add(&mut self, path: &[String], depth: usize) {
        if depth == path.len() {
            self.leaf = true;
            return;
        }
        let name = &path[depth];
        let at = match self.children.iter().position(|child| &child.name == name) {
            Some(at) => at,
            None => {
                self.children.push(Node {
                    name: name.clone(),
                    path: path[..=depth].to_vec(),
                    ..Default::default()
                });
                self.children.len() - 1
            }
        };
        self.children[at].add(path, depth + 1);
    }

    /// Leaves at or under this node.
    pub fn leaves<'a>(&'a self, into: &mut Vec<&'a Node>) {
        if self.leaf {
            into.push(self);
        }
        for child in &self.children {
            child.leaves(into);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> KeyPath {
        parse_path(text).unwrap()
    }

    fn shipped(code: &str) -> Option<String> {
        std::fs::read_to_string(format!("assets/default/lang/{code}.toml")).ok()
    }

    /// Fails if opening and saving a file without editing changes a byte of it.
    #[test]
    fn every_shipped_file_survives_a_save_unchanged() {
        for code in ["en-US", "de-DE"] {
            let Some(text) = shipped(code) else { return };
            let doc = LangDoc::parse(&text).unwrap();
            assert_eq!(doc.text(), text, "{code} changed on a round trip");
        }
    }

    /// Fails if an edit reorders, reformats or drops anything but the line it changes.
    #[test]
    fn editing_a_value_changes_only_its_line() {
        let Some(text) = shipped("en-US") else { return };
        let mut doc = LangDoc::parse(&text).unwrap();
        doc.set(&path("debug.movement.perspective.speed"), "Fast").unwrap();
        doc.set(&path("tools.select"), "Pick { thing }").unwrap();
        let edited = doc.text();

        let before: Vec<&str> = text.lines().collect();
        let after: Vec<&str> = edited.lines().collect();
        assert_eq!(before.len(), after.len());
        let changed: Vec<(&str, &str)> =
            before.iter().zip(&after).filter(|(a, b)| a != b).map(|(a, b)| (*a, *b)).collect();
        assert_eq!(changed, vec![
            ("perspective.speed = \"Perspective camera speed\"", "perspective.speed = \"Fast\""),
            ("select = \"Select\"", "select = \"Pick { thing }\""),
        ]);

        let reread = LangDoc::parse(&edited).unwrap();
        assert_eq!(reread.leaf(&path("tools.select")), Some(Leaf::Text("Pick { thing }".into())));
        assert_eq!(
            doc.leaves().iter().map(|(path, _)| path.clone()).collect::<Vec<_>>(),
            LangDoc::parse(&text).unwrap().leaves().iter().map(|(path, _)| path.clone()).collect::<Vec<_>>(),
            "the keys moved",
        );
    }

    const SMALL: &str = "\
[language]
name = \"Test\"

[a]
one = \"1\" # kept
two = \"2\"
dot.x = \"x\"
dot.y = \"y\"

[a.deep]
three = \"3\"

[b]
four = \"4\"
";

    #[test]
    fn a_new_key_goes_at_the_end_of_its_table() {
        let mut doc = LangDoc::parse(SMALL).unwrap();
        doc.insert(&path("a.new"), "N").unwrap();
        doc.insert(&path("a.dot.z"), "Z").unwrap();
        let text = doc.text();
        assert!(text.contains("dot.y = \"y\"\ndot.z = \"Z\"\nnew = \"N\"\n\n[a.deep]"), "{text}");
    }

    #[test]
    fn a_new_table_lands_after_its_parents_tables() {
        let mut doc = LangDoc::parse(SMALL).unwrap();
        doc.insert(&path("a.fresh.leaf"), "L").unwrap();
        doc.insert(&path("c.d.e"), "E").unwrap();
        let text = doc.text();
        assert!(text.contains("three = \"3\"\n\n[a.fresh]\nleaf = \"L\"\n\n[b]"), "{text}");
        assert!(text.ends_with("four = \"4\"\n\n[c.d]\ne = \"E\"\n"), "{text}");
        assert!(!text.contains("[c]\n"), "an empty parent header was written: {text}");
        LangDoc::parse(&text).unwrap();
    }

    #[test]
    fn a_key_cannot_be_both_a_value_and_a_table() {
        let mut doc = LangDoc::parse(SMALL).unwrap();
        assert_eq!(doc.insert(&path("a.one.sub"), "x"), Err(LangError::NotATable("a.one".into())));
        assert_eq!(doc.insert(&path("a.one"), "x"), Err(LangError::Exists("a.one".into())));
        assert_eq!(doc.text(), SMALL);
    }

    #[test]
    fn renaming_keeps_the_place_and_the_comment() {
        let mut doc = LangDoc::parse(SMALL).unwrap();
        doc.rename(&path("a.one"), "uno").unwrap();
        doc.rename(&path("a.deep"), "abyss").unwrap();
        doc.rename(&path("a.dot"), "spot").unwrap();
        let text = doc.text();
        assert!(text.contains("[a]\nuno = \"1\" # kept\ntwo = \"2\"\nspot.x = \"x\"\nspot.y = \"y\"\n\n[a.abyss]\nthree"), "{text}");
        assert_eq!(doc.rename(&path("a.two"), "uno"), Err(LangError::Exists("a.uno".into())));
        assert_eq!(doc.rename(&path("a.two"), "has.dot"), Err(LangError::BadKey));
    }

    #[test]
    fn removing_the_last_key_takes_its_table() {
        let mut doc = LangDoc::parse(SMALL).unwrap();
        assert!(doc.remove(&path("a.deep.three")));
        assert!(!doc.text().contains("[a.deep]"));
        assert!(doc.remove(&path("a.dot")));
        assert!(!doc.text().contains("dot."));
        assert!(doc.remove(&path("b")));
        assert!(!doc.remove(&path("b")));
        assert_eq!(doc.text(), "[language]\nname = \"Test\"\n\n[a]\none = \"1\" # kept\ntwo = \"2\"\n");
    }

    #[test]
    fn marks_follow_the_reference() {
        let reference: toml::Table = toml::from_str("[a]\nx = \"Hi { name }\"\ny = \"Y\"").unwrap();
        let reference = Reference::from_table(&reference);
        let at = |key: &str| path(key);

        assert!(Marks::of(None, Some(&reference), &at("a.x")).missing);
        assert!(Marks::of(Some("Hallo"), Some(&reference), &at("a.x")).placeholders);
        assert!(!Marks::of(Some("Hallo {name}"), Some(&reference), &at("a.x")).any());
        let stub = Marks::of(Some("MISSING a.x"), Some(&reference), &at("a.x"));
        assert!(stub.stub && !stub.placeholders);
        assert!(Marks::of(Some("z"), Some(&reference), &at("a.z")).extra);
        assert!(!Marks::of(Some("Hallo"), None, &at("a.x")).any(), "the reference checked against itself");
    }

    #[test]
    fn the_tree_puts_reference_only_keys_under_their_parents() {
        let doc = LangDoc::parse("[a]\ny = \"Y\"\n").unwrap();
        let reference: toml::Table = toml::from_str("[a]\nx = \"X\"\ny = \"Y\"\n[b]\nz = \"Z\"").unwrap();
        let tree = Node::build(&doc, Some(&Reference::from_table(&reference)));
        let names = |node: &Node| node.children.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&tree), ["a", "b"]);
        assert_eq!(names(&tree.children[0]), ["y", "x"]);
        assert!(tree.children[0].children[1].leaf);
    }
}
