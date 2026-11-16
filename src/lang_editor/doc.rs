//! A lang file as `toml_edit` holds it, so an edit touches only the lines it changes: order,
//! comments, blank lines, dotted keys and `[headers]` all survive a save.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};

use toml_edit::{Array, DocumentMut, Item, Key, Table, TableLike, Value};

pub type KeyPath = Vec<String>;

pub fn dotted(path: &[String]) -> String {
    path.join(".")
}

/// `get!` splits on `.`, so a segment cannot hold one; the rest keeps keys bare in the file.
pub fn valid_segment(segment: &str) -> bool {
    !segment.is_empty() && segment.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

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
    /// The key holds something other than what the edit is for: text, or a list of text.
    WrongKind(String),
}

impl LangError {
    pub fn message(&self) -> String {
        match self {
            LangError::BadKey => crate::get!("lang_editor.error.bad_key"),
            LangError::Exists(key) => crate::get!("lang_editor.error.exists", "key", key),
            LangError::NotATable(key) => crate::get!("lang_editor.error.not_a_table", "key", key),
            LangError::Missing(key) => crate::get!("lang_editor.error.missing", "key", key),
            LangError::WrongKind(key) => crate::get!("lang_editor.error.wrong_kind", "key", key),
        }
    }
}

/// Strings and arrays of strings are edited; anything else is shown as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Leaf {
    Text(String),
    /// One is picked by `get_random!`; `get!` takes the first.
    List(Vec<String>),
    Other(String),
}

impl Leaf {
    /// A list's entries one per line, so its slots and a filter see all of them.
    pub fn text(&self) -> Cow<'_, str> {
        match self {
            Leaf::Text(text) | Leaf::Other(text) => Cow::Borrowed(text),
            Leaf::List(items) => Cow::Owned(items.join("\n")),
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

    /// In file order.
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

    fn array_mut(&mut self, path: &[String]) -> Result<&mut Array, LangError> {
        let (last, parents) = path.split_last().ok_or(LangError::BadKey)?;
        match self.table_mut(parents).and_then(|table| table.get_mut(last)) {
            Some(Item::Value(Value::Array(array))) => Ok(array),
            Some(_) => Err(LangError::WrongKind(dotted(path))),
            None => Err(LangError::Missing(dotted(path))),
        }
    }

    pub fn set_item(&mut self, path: &[String], index: usize, text: &str) -> Result<(), LangError> {
        let array = self.array_mut(path)?;
        if index >= array.len() {
            return Err(LangError::Missing(format!("{}[{index}]", dotted(path))));
        }
        array.replace(index, text);
        Ok(())
    }

    /// Laid out as the last entry is, so a list written one entry per line stays that way.
    pub fn push_item(&mut self, path: &[String], text: &str) -> Result<(), LangError> {
        let array = self.array_mut(path)?;
        let Some(last) = array.iter().last() else {
            array.push(text);
            return Ok(());
        };
        // A lone inline entry has no space before it, which a second one needs.
        let one_per_line = last.decor().prefix().and_then(|prefix| prefix.as_str()).is_some_and(|prefix| prefix.contains('\n'));
        if array.len() == 1 && !one_per_line {
            array.push(text);
            return Ok(());
        }
        let decor = last.decor().clone();
        let mut fresh = Value::from(text);
        *fresh.decor_mut() = decor;
        array.push_formatted(fresh);
        Ok(())
    }

    /// The last entry stays: a list of none is nothing to pick from.
    pub fn remove_item(&mut self, path: &[String], index: usize) -> Result<(), LangError> {
        let array = self.array_mut(path)?;
        if index >= array.len() || array.len() == 1 {
            return Err(LangError::Missing(format!("{}[{index}]", dotted(path))));
        }
        let was_first = index == 0;
        let first_decor = array.get(0).unwrap().decor().clone();
        array.remove(index);
        // The new first entry takes the old first's place after the `[`.
        if was_first {
            *array.get_mut(0).unwrap().decor_mut() = first_decor;
        }
        Ok(())
    }

    /// Text becomes a list of that one entry, or a list of one entry becomes its text, keeping
    /// the key's spacing and comment.
    pub fn toggle_list(&mut self, path: &[String]) -> Result<(), LangError> {
        let (last, parents) = path.split_last().ok_or(LangError::BadKey)?;
        let Some(Item::Value(value)) = self.table_mut(parents).and_then(|table| table.get_mut(last)) else {
            return Err(LangError::Missing(dotted(path)));
        };
        let decor = value.decor().clone();
        let mut fresh = match value {
            Value::String(text) => Value::Array(Array::from_iter([text.value().as_str()])),
            Value::Array(array) if array.len() == 1 => match array.get(0).and_then(Value::as_str) {
                Some(text) => Value::from(text),
                None => return Err(LangError::WrongKind(dotted(path))),
            },
            _ => return Err(LangError::WrongKind(dotted(path))),
        };
        *fresh.decor_mut() = decor;
        *value = fresh;
        Ok(())
    }

    /// As [`LangDoc::insert`], for a list.
    pub fn insert_list(&mut self, path: &[String], items: &[String]) -> Result<(), LangError> {
        self.insert_value(path, Value::Array(Array::from_iter(items.iter().map(String::as_str))))
    }

    /// At the end of its table. Missing tables are made dotted under a dotted table and as
    /// `[headers]` otherwise; `toml_edit` writes a table with no position after its parent's last.
    pub fn insert(&mut self, path: &[String], text: &str) -> Result<(), LangError> {
        self.insert_value(path, Value::from(text))
    }

    fn insert_value(&mut self, path: &[String], value: Value) -> Result<(), LangError> {
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
        table.insert(last, Item::Value(value));
        Ok(())
    }

    /// Renames the last segment of `path`, keeping its place.
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

    /// Also removes any table this leaves empty.
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
        Value::Array(array) if array.iter().all(Value::is_str) => {
            Leaf::List(array.iter().filter_map(Value::as_str).map(str::to_string).collect())
        }
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
    /// A list's entries one per line, as [`Leaf::text`].
    values: HashMap<KeyPath, String>,
    lists: HashMap<KeyPath, Vec<String>>,
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

    pub fn list(&self, path: &[String]) -> Option<&[String]> {
        self.lists.get(path).map(Vec::as_slice)
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
            toml::Value::Array(items) if items.iter().all(toml::Value::is_str) => {
                let items: Vec<String> = items.iter().filter_map(toml::Value::as_str).map(str::to_string).collect();
                into.order.push(path.clone());
                into.values.insert(path.clone(), items.join("\n"));
                into.lists.insert(path.clone(), items);
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
    /// Text where the reference has a list, or a list where it has text.
    pub kind: bool,
}

impl Marks {
    pub fn any(self) -> bool {
        self.missing || self.stub || self.placeholders || self.extra || self.kind
    }

    /// `reference` is `None` when the open file is the reference itself. Each entry of a list
    /// must have every slot the reference uses, since any one of them may be picked.
    pub fn of(value: Option<&Leaf>, reference: Option<&Reference>, path: &[String]) -> Marks {
        let entries: Vec<&str> = match value {
            Some(Leaf::Text(text) | Leaf::Other(text)) => vec![text],
            Some(Leaf::List(items)) => items.iter().map(String::as_str).collect(),
            None => Vec::new(),
        };
        let stub = entries.iter().any(|entry| entry.starts_with(STUB_PREFIX));
        let Some(reference) = reference else { return Marks { stub, ..Default::default() } };
        let wanted = reference.get(path);
        Marks {
            missing: value.is_none() && wanted.is_some(),
            stub,
            placeholders: match wanted {
                Some(wanted) if !stub => entries.iter().any(|entry| placeholders(entry) != placeholders(wanted)),
                _ => false,
            },
            extra: value.is_some() && wanted.is_none(),
            kind: wanted.is_some()
                && match value {
                    Some(Leaf::Text(_)) => reference.list(path).is_some(),
                    Some(Leaf::List(_)) => reference.list(path).is_none(),
                    _ => false,
                },
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
    /// `path` joined, kept since every visible row asks for it every frame.
    pub dotted: String,
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
                    dotted: dotted(&path[..=depth]),
                    ..Default::default()
                });
                self.children.len() - 1
            }
        };
        self.children[at].add(path, depth + 1);
    }

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
        assert!(Marks::of(Some(&Leaf::Text("Hallo".into())), Some(&reference), &at("a.x")).placeholders);
        assert!(!Marks::of(Some(&Leaf::Text("Hallo {name}".into())), Some(&reference), &at("a.x")).any());
        let stub = Marks::of(Some(&Leaf::Text("MISSING a.x".into())), Some(&reference), &at("a.x"));
        assert!(stub.stub && !stub.placeholders);
        assert!(Marks::of(Some(&Leaf::Text("z".into())), Some(&reference), &at("a.z")).extra);
        assert!(!Marks::of(Some(&Leaf::Text("Hallo".into())), None, &at("a.x")).any(), "the reference checked against itself");
    }

    const LISTS: &str = "\
[a]
inline = [\"x\", \"y\"]
tall = [
    \"one\",
    \"two\",
] # kept
plain = \"p\"
";

    #[test]
    fn a_list_of_text_is_a_list_leaf() {
        let doc = LangDoc::parse(LISTS).unwrap();
        assert_eq!(doc.leaf(&path("a.tall")), Some(Leaf::List(vec!["one".into(), "two".into()])));
        assert_eq!(doc.leaf(&path("a.tall")).unwrap().text(), "one\ntwo");
        let mixed = LangDoc::parse("a = [\"x\", 1]").unwrap();
        assert!(matches!(mixed.leaf(&path("a")), Some(Leaf::Other(_))));
    }

    /// Fails if editing, adding or removing an entry reformats the list or touches another line.
    #[test]
    fn editing_a_list_keeps_its_layout() {
        let mut doc = LangDoc::parse(LISTS).unwrap();
        doc.set_item(&path("a.tall"), 1, "deux").unwrap();
        doc.push_item(&path("a.tall"), "three").unwrap();
        doc.push_item(&path("a.inline"), "z").unwrap();
        let mut lone = LangDoc::parse("x = [\n    \"one\",\n]\n").unwrap();
        lone.push_item(&path("x"), "two").unwrap();
        assert_eq!(lone.text(), "x = [\n    \"one\",\n    \"two\",\n]\n");
        assert_eq!(doc.text(), "[a]\ninline = [\"x\", \"y\", \"z\"]\ntall = [\n    \"one\",\n    \"deux\",\n    \"three\",\n] # kept\nplain = \"p\"\n");

        doc.remove_item(&path("a.tall"), 0).unwrap();
        doc.remove_item(&path("a.inline"), 0).unwrap();
        assert_eq!(doc.text(), "[a]\ninline = [\"y\", \"z\"]\ntall = [\n    \"deux\",\n    \"three\",\n] # kept\nplain = \"p\"\n");
        assert!(doc.set_item(&path("a.plain"), 0, "q").is_err());
    }

    #[test]
    fn the_last_entry_of_a_list_stays() {
        let mut doc = LangDoc::parse("a = [\"only\"]\n").unwrap();
        assert!(doc.remove_item(&path("a"), 0).is_err());
        assert_eq!(doc.text(), "a = [\"only\"]\n");
    }

    #[test]
    fn text_and_a_list_of_one_turn_into_each_other() {
        let mut doc = LangDoc::parse("a = \"x\" # kept\nb = [\"p\", \"q\"]\n").unwrap();
        doc.toggle_list(&path("a")).unwrap();
        assert_eq!(doc.text(), "a = [\"x\"] # kept\nb = [\"p\", \"q\"]\n");
        doc.toggle_list(&path("a")).unwrap();
        assert_eq!(doc.text(), "a = \"x\" # kept\nb = [\"p\", \"q\"]\n");
        assert_eq!(doc.toggle_list(&path("b")), Err(LangError::WrongKind("b".into())));
    }

    #[test]
    fn a_list_in_the_reference_is_checked_and_copied_whole() {
        let reference: toml::Table = toml::from_str("[a]\nx = [\"Hi { name }\", \"Yo { name }\"]").unwrap();
        let reference = Reference::from_table(&reference);
        assert_eq!(reference.list(&path("a.x")), Some(&["Hi { name }".to_string(), "Yo { name }".to_string()][..]));
        let list = |items: &[&str]| Leaf::List(items.iter().map(|item| item.to_string()).collect());
        assert!(Marks::of(Some(&list(&["Hallo { name }", "Hallo"])), Some(&reference), &path("a.x")).placeholders, "one entry dropped a slot");
        assert!(!Marks::of(Some(&list(&["Hallo { name }", "Tag { name }"])), Some(&reference), &path("a.x")).any());
        assert!(Marks::of(Some(&Leaf::Text("Hallo { name }".into())), Some(&reference), &path("a.x")).kind, "text where en-US has a list");

        let mut doc = LangDoc::parse("[a]\n").unwrap();
        doc.insert_list(&path("a.x"), reference.list(&path("a.x")).unwrap()).unwrap();
        assert_eq!(doc.leaf(&path("a.x")), Some(Leaf::List(vec!["Hi { name }".into(), "Yo { name }".into()])));
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
