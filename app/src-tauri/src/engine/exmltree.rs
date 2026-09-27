//! A mutable EXML tree, so a merge can be written as well as measured.
//!
//! [`super::exml`] flattens a document into `path -> value` for comparison and
//! uses `roxmltree`, which is read-only by design. Producing a merged asset
//! needs the other direction: set a value at a path, graft a subtree that one
//! copy has and another lacks, and write the result back out for MBINCompiler.
//!
//! **The paths must match the analyser's exactly.** Edits are addressed by the
//! same `Table[LAUNCHER]/Cost` strings the conflict engine produces, so a
//! disagreement between the two path builders would silently drop edits on the
//! floor rather than fail. The segment rules are therefore reproduced here in
//! one place, [`Node::segment`], and `paths_match_the_analyser` checks the two
//! against each other on the same document.

use std::collections::HashMap;
use std::fmt::Write as _;

use indexmap::IndexMap;

use super::exml::ID_KEYS;

/// One `<Property>` element.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Node {
    /// the `name` attribute, which is what the path is built from
    pub name: String,
    pub value: Option<String>,
    /// every other attribute, in document order: `_id`, `_index`, ...
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl Node {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Stable key for a table row, taken from its Id/Name-ish child.
    fn identity(&self) -> Option<&str> {
        self.children.iter().find_map(|child| {
            let name = child.name.to_uppercase();
            if ID_KEYS.contains(&name.as_str()) {
                child.value.as_deref().filter(|v| !v.is_empty())
            } else {
                None
            }
        })
    }

    /// Path segment for this node. Mirrors `exml::segment`.
    fn segment(&self, position: usize, repeated: bool) -> String {
        if let Some(id) = self.attr("_id") {
            if !id.is_empty() {
                return format!("{}[{id}]", self.name);
            }
        }
        if let Some(ident) = self.identity() {
            return format!("{}[{ident}]", self.name);
        }
        if let Some(index) = self.attr("_index") {
            return format!("{}[{index}]", self.name);
        }
        if !repeated {
            return self.name.clone();
        }
        format!("{}[{position}]", self.name)
    }
}

/// A whole `<Data template="...">` document.
#[derive(Debug, Clone, Default)]
pub struct Tree {
    /// Comment lines above the root, including the MBINCompiler version stamp
    /// that the compiler reads back. Dropping these breaks recompilation.
    pub header: Vec<String>,
    pub template: Option<String>,
    pub children: Vec<Node>,
}

/// Address of one node: the child index to take at each level.
type Route = Vec<usize>;

impl Tree {
    /// Every node's path, in document order, paired with its route.
    pub fn index(&self) -> IndexMap<String, Route> {
        let mut out = IndexMap::new();
        Self::walk(&self.children, "", &mut Vec::new(), &mut out);
        out
    }

    fn walk(
        nodes: &[Node],
        prefix: &str,
        route: &mut Route,
        out: &mut IndexMap<String, Route>,
    ) {
        // Group by `name`, preserving first-seen order, exactly as the
        // analyser does -- `repeated` depends on it.
        let mut groups: IndexMap<&str, Vec<usize>> = IndexMap::new();
        for (i, node) in nodes.iter().enumerate() {
            groups.entry(node.name.as_str()).or_default().push(i);
        }

        for (_name, kids) in groups {
            let repeated = kids.len() > 1;
            let mut seen: HashMap<String, usize> = HashMap::new();
            for (position, i) in kids.into_iter().enumerate() {
                let node = &nodes[i];
                let mut seg = node.segment(position, repeated);
                let count = seen.entry(seg.clone()).or_insert(0);
                let n = *count;
                *count += 1;
                if n > 0 {
                    seg = format!("{seg}#{n}");
                }
                let path = if prefix.is_empty() {
                    seg
                } else {
                    format!("{prefix}/{seg}")
                };
                route.push(i);
                out.insert(path.clone(), route.clone());
                Self::walk(&node.children, &path, route, out);
                route.pop();
            }
        }
    }

    fn at_mut(&mut self, route: &[usize]) -> Option<&mut Node> {
        let (first, rest) = route.split_first()?;
        let mut node = self.children.get_mut(*first)?;
        for step in rest {
            node = node.children.get_mut(*step)?;
        }
        Some(node)
    }

    fn at(&self, route: &[usize]) -> Option<&Node> {
        let (first, rest) = route.split_first()?;
        let mut node = self.children.get(*first)?;
        for step in rest {
            node = node.children.get(*step)?;
        }
        Some(node)
    }

    /// Set the value at `path`, returning false when the path is absent.
    pub fn set(&mut self, index: &IndexMap<String, Route>, path: &str, value: Option<&str>) -> bool {
        let Some(route) = index.get(path) else {
            return false;
        };
        let route = route.clone();
        match self.at_mut(&route) {
            Some(node) => {
                node.value = value.map(str::to_string);
                true
            }
            None => false,
        }
    }

    /// Append `node` under `parent_path`, or at the root when it is empty.
    pub fn graft(
        &mut self,
        index: &IndexMap<String, Route>,
        parent_path: &str,
        node: Node,
    ) -> bool {
        if parent_path.is_empty() {
            self.children.push(node);
            return true;
        }
        let Some(route) = index.get(parent_path) else {
            return false;
        };
        let route = route.clone();
        match self.at_mut(&route) {
            Some(parent) => {
                parent.children.push(node);
                true
            }
            None => false,
        }
    }

    /// The node at `path`, cloned, for grafting into another tree.
    pub fn take(&self, index: &IndexMap<String, Route>, path: &str) -> Option<Node> {
        self.at(index.get(path)?).cloned()
    }

    /// The value at `path`, if the path exists.
    pub fn value_at(&self, index: &IndexMap<String, Route>, path: &str) -> Option<String> {
        self.at(index.get(path)?)?.value.clone()
    }

    /// Flatten to `path -> value`, matching [`super::exml::parse`].
    pub fn flatten(&self) -> IndexMap<String, Option<String>> {
        let index = self.index();
        index
            .keys()
            .map(|path| (path.clone(), self.value_at(&index, path)))
            .collect()
    }

    /// A copy holding only `keep` and the ancestors leading to it.
    ///
    /// Rows that survive carry their identity forward as an `_id` attribute,
    /// because the `Id` child that expressed it is usually pruned away. The
    /// game, and this engine's own path keying, both read `_id` first, so the
    /// pruned row addresses the same position as the full one.
    pub fn retain(&self, keep: &std::collections::HashSet<String>) -> Tree {
        let mut out = Tree {
            header: self.header.clone(),
            template: self.template.clone(),
            children: Vec::new(),
        };
        out.children = Self::retain_in(&self.children, "", keep);
        out
    }

    fn retain_in(
        nodes: &[Node],
        prefix: &str,
        keep: &std::collections::HashSet<String>,
    ) -> Vec<Node> {
        let mut groups: IndexMap<&str, Vec<usize>> = IndexMap::new();
        for (i, node) in nodes.iter().enumerate() {
            groups.entry(node.name.as_str()).or_default().push(i);
        }

        let mut out: Vec<(usize, Node)> = Vec::new();
        for (_name, kids) in groups {
            let repeated = kids.len() > 1;
            let mut seen: HashMap<String, usize> = HashMap::new();
            for (position, i) in kids.into_iter().enumerate() {
                let node = &nodes[i];
                let mut seg = node.segment(position, repeated);
                let count = seen.entry(seg.clone()).or_insert(0);
                let n = *count;
                *count += 1;
                if n > 0 {
                    seg = format!("{seg}#{n}");
                }
                let path = if prefix.is_empty() {
                    seg
                } else {
                    format!("{prefix}/{seg}")
                };

                let children = Self::retain_in(&node.children, &path, keep);
                let wanted = keep.contains(&path);
                if !wanted && children.is_empty() {
                    continue;
                }

                let mut kept = Node {
                    name: node.name.clone(),
                    value: node.value.clone(),
                    attrs: node.attrs.clone(),
                    children,
                };
                // Carry the identity forward before its source disappears.
                if let Some(ident) = node.identity() {
                    if kept.attr("_id").is_none() {
                        let ident = ident.to_string();
                        kept.attrs.insert(0, ("_id".to_string(), ident));
                    }
                }
                // A member of a list that has no id of any kind is addressed
                // by where it sits, and pruning its siblings moves it: the
                // third of five becomes the only one, and both this engine
                // and the game would then read it as the first. `_index` says
                // the position outright, which is what it is for -- the
                // hand-verified sparse patch this feature is modelled on uses
                // it for exactly this.
                if repeated && kept.attr("_id").is_none() && kept.attr("_index").is_none() {
                    kept.attrs
                        .insert(0, ("_index".to_string(), position.to_string()));
                }
                out.push((i, kept));
            }
        }
        // Restore document order; the grouping above visits by name.
        out.sort_by_key(|(i, _)| *i);
        out.into_iter().map(|(_, node)| node).collect()
    }

    /// Renumber `_index` attributes so they match document order again.
    ///
    /// MBINCompiler writes `_index` on array members. Grafting a node into an
    /// array leaves the numbering describing a different array than the file
    /// now contains, so it has to be rewritten before recompiling.
    pub fn renumber(&mut self) {
        Self::renumber_in(&mut self.children);
    }

    fn renumber_in(nodes: &mut [Node]) {
        let mut groups: IndexMap<String, Vec<usize>> = IndexMap::new();
        for (i, node) in nodes.iter().enumerate() {
            groups.entry(node.name.clone()).or_default().push(i);
        }
        for (_name, kids) in groups {
            for (position, i) in kids.into_iter().enumerate() {
                if let Some(slot) = nodes[i].attrs.iter_mut().find(|(k, _)| k == "_index") {
                    slot.1 = position.to_string();
                }
            }
        }
        for node in nodes.iter_mut() {
            Self::renumber_in(&mut node.children);
        }
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

fn convert(node: roxmltree::Node) -> Node {
    let mut out = Node {
        name: node.attribute("name").unwrap_or("?").to_string(),
        value: node.attribute("value").map(str::to_string),
        ..Default::default()
    };
    for attr in node.attributes() {
        if attr.name() != "name" && attr.name() != "value" {
            out.attrs
                .push((attr.name().to_string(), attr.value().to_string()));
        }
    }
    for child in node.children().filter(|c| c.is_element()) {
        if child.has_tag_name("Property") {
            out.children.push(convert(child));
        }
    }
    out
}

/// Read a document into a mutable tree.
pub fn parse_str(text: &str) -> Result<Tree, String> {
    let parsed = roxmltree::Document::parse(text)
        .map_err(|err| format!("XML parse error: {err}"))?;
    let root = parsed.root_element();

    let mut tree = Tree {
        template: root.attribute("template").map(str::to_string),
        ..Default::default()
    };
    // Keep the comments above the root: MBINCompiler reads its own version
    // stamp back out of them, so a file without them will not recompile.
    for node in parsed.root().children() {
        if node.is_comment() {
            if let Some(text) = node.text() {
                tree.header.push(text.to_string());
            }
        }
    }
    for child in root.children().filter(|c| c.is_element()) {
        if child.has_tag_name("Property") {
            tree.children.push(convert(child));
        }
    }
    Ok(tree)
}

pub fn parse(path: &std::path::Path) -> Result<Tree, String> {
    let text = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    parse_str(&text)
}

fn write_node(out: &mut String, node: &Node, depth: usize) {
    let pad = "  ".repeat(depth);
    let _ = write!(out, "{pad}<Property name=\"{}\"", escape(&node.name));
    if let Some(value) = &node.value {
        let _ = write!(out, " value=\"{}\"", escape(value));
    }
    for (key, value) in &node.attrs {
        let _ = write!(out, " {key}=\"{}\"", escape(value));
    }
    if node.children.is_empty() {
        out.push_str(" />\n");
        return;
    }
    out.push_str(">\n");
    for child in &node.children {
        write_node(out, child, depth + 1);
    }
    let _ = writeln!(out, "{pad}</Property>");
}

/// Serialise back to EXML, ready for MBINCompiler.
pub fn to_string(tree: &Tree) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    for comment in &tree.header {
        let _ = writeln!(out, "<!--{comment}-->");
    }
    match &tree.template {
        Some(t) => {
            let _ = writeln!(out, "<Data template=\"{}\">", escape(t));
        }
        None => out.push_str("<Data>\n"),
    }
    for child in &tree.children {
        write_node(&mut out, child, 1);
    }
    out.push_str("</Data>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
        <!--File created using MBINCompiler version (7.03.2.2)-->\n\
        <Data template=\"cGcTable\">\
        <Property name=\"Row\" value=\"E\" _index=\"0\">\
        <Property name=\"Id\" value=\"ALPHA\" />\
        <Property name=\"Cost\" value=\"1\" />\
        </Property>\
        <Property name=\"Row\" value=\"E\" _index=\"1\">\
        <Property name=\"Id\" value=\"BETA\" />\
        <Property name=\"Cost\" value=\"2\" />\
        </Property>\
        </Data>";

    /// The merge addresses edits by the analyser's paths. If these two ever
    /// disagree, edits land nowhere and the merge silently loses them.
    #[test]
    fn paths_match_the_analyser() {
        let tree = parse_str(DOC).unwrap();
        let index = tree.index();
        let mine: Vec<&String> = index.keys().collect();
        let analysed = super::super::exml::parse_str_for_test(DOC);
        let theirs: Vec<&String> = analysed.props.keys().collect();
        assert_eq!(mine, theirs);
    }

    /// Real UI assets (`HUDCROSSHAIR.MBIN`) carry no `_index` at all: every
    /// list member is addressed by where it sits. Keeping the third of three
    /// leaves it an only child, and nothing would say it was ever the third.
    #[test]
    fn a_positional_list_member_keeps_its_position() {
        const NESTED: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
            <!--File created using MBINCompiler version (7.03.2.2)-->\n\
            <Data template=\"cGcLayer\">\
            <Property name=\"Children\">\
            <Property name=\"Children\"><Property name=\"Hidden\" value=\"false\" /></Property>\
            <Property name=\"Children\"><Property name=\"Hidden\" value=\"false\" /></Property>\
            <Property name=\"Children\"><Property name=\"Hidden\" value=\"true\" /></Property>\
            </Property>\
            </Data>";

        let tree = parse_str(NESTED).unwrap();
        let wanted = "Children/Children[2]/Hidden";
        assert!(tree.flatten().contains_key(wanted));

        let mut keep = std::collections::HashSet::new();
        for path in ["Children", "Children/Children[2]", wanted] {
            keep.insert(path.to_string());
        }
        let pruned = tree.retain(&keep);

        assert_eq!(
            pruned.flatten().get(wanted),
            Some(&Some("true".to_string())),
            "the survivor must still answer to the path it had"
        );
        assert!(
            to_string(&pruned).contains("_index=\"2\""),
            "and must tell the game which slot it is, since nothing else can"
        );
    }

    #[test]
    fn a_round_trip_preserves_the_document() {
        let tree = parse_str(DOC).unwrap();
        let again = parse_str(&to_string(&tree)).unwrap();
        assert_eq!(tree.children, again.children);
        assert_eq!(tree.template, again.template);
        assert!(
            to_string(&tree).contains("MBINCompiler version (7.03.2.2)"),
            "the version stamp must survive or the file will not recompile"
        );
    }

    #[test]
    fn setting_a_value_lands_at_the_right_row() {
        let mut tree = parse_str(DOC).unwrap();
        let index = tree.index();
        assert!(tree.set(&index, "Row[BETA]/Cost", Some("99")));
        assert!(!tree.set(&index, "Row[NOPE]/Cost", Some("1")), "unknown path");
        let after = parse_str(&to_string(&tree)).unwrap();
        let index = after.index();
        assert_eq!(
            after.at(&index["Row[BETA]/Cost"]).unwrap().value.as_deref(),
            Some("99")
        );
        assert_eq!(
            after.at(&index["Row[ALPHA]/Cost"]).unwrap().value.as_deref(),
            Some("1")
        );
    }

    #[test]
    fn grafting_a_row_renumbers_the_array() {
        let mut tree = parse_str(DOC).unwrap();
        let index = tree.index();
        let donor = tree.take(&index, "Row[BETA]").unwrap();
        let mut new_row = donor.clone();
        new_row.children[0].value = Some("GAMMA".to_string());
        assert!(tree.graft(&index, "", new_row));
        tree.renumber();

        let after = parse_str(&to_string(&tree)).unwrap();
        let index = after.index();
        assert!(index.contains_key("Row[GAMMA]"));
        // _index now describes the array the file actually contains.
        let indices: Vec<&str> = after
            .children
            .iter()
            .filter_map(|n| n.attr("_index"))
            .collect();
        assert_eq!(indices, ["0", "1", "2"]);
    }

    #[test]
    fn an_inserted_row_does_not_move_the_others() {
        let mut tree = parse_str(DOC).unwrap();
        let index = tree.index();
        let mut extra = tree.take(&index, "Row[ALPHA]").unwrap();
        extra.children[0].value = Some("OMEGA".to_string());
        tree.children.insert(0, extra);
        tree.renumber();

        let after = parse_str(&to_string(&tree)).unwrap();
        let index = after.index();
        // Keyed by identity, ALPHA and BETA are where they always were.
        assert!(index.contains_key("Row[ALPHA]/Cost"));
        assert!(index.contains_key("Row[BETA]/Cost"));
        assert_eq!(
            after.at(&index["Row[BETA]/Cost"]).unwrap().value.as_deref(),
            Some("2")
        );
    }

    #[test]
    fn special_characters_survive_a_round_trip() {
        let doc = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
            <Data template=\"T\">\
            <Property name=\"Name\" value=\"Astro &amp; Babs &lt;Bridge&gt;\" />\
            </Data>";
        let tree = parse_str(doc).unwrap();
        let again = parse_str(&to_string(&tree)).unwrap();
        assert_eq!(
            again.children[0].value.as_deref(),
            Some("Astro & Babs <Bridge>")
        );
    }
}
