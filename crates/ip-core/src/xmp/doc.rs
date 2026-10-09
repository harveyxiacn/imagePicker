//! A small, loss-tolerant XMP document model.
//!
//! XMP is RDF/XML. This is not a general XML library: it parses just enough to find the
//! `rdf:Description` elements and the handful of properties imagePicker syncs (rating, label,
//! keywords) in both attribute and element form, and it writes the document back with everything
//! it does not understand intact (other namespaces, history, develop settings, comments, the
//! `xpacket` wrapper). Unchanged elements keep their attribute order and text; only whitespace
//! inside tags is normalised.

use std::fmt;

pub const NS_RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
pub const NS_XMP: &str = "http://ns.adobe.com/xap/1.0/";
pub const NS_DC: &str = "http://purl.org/dc/elements/1.1/";
pub const NS_LR: &str = "http://ns.adobe.com/lightroom/1.0/";
pub const NS_XMPDM: &str = "http://ns.adobe.com/xmp/1.0/DynamicMedia/";
pub const NS_DARKTABLE: &str = "http://darktable.sf.net/";
pub const NS_TIFF: &str = "http://ns.adobe.com/tiff/1.0/";
pub const NS_EXIF: &str = "http://ns.adobe.com/exif/1.0/";
pub const NS_CRS: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";
pub const NS_XMP_NOTE: &str = "http://ns.adobe.com/xmp/note/";
const NS_XML: &str = "http://www.w3.org/XML/1998/namespace";
const NS_META: &str = "adobe:ns:meta/";

#[derive(Debug)]
pub struct XmpError(pub String);

impl fmt::Display for XmpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid XMP: {}", self.0)
    }
}

impl std::error::Error for XmpError {}

type Res<T> = Result<T, XmpError>;

fn err<T>(m: impl Into<String>) -> Res<T> {
    Err(XmpError(m.into()))
}

#[derive(Debug, Clone, PartialEq)]
struct Attr {
    qname: String,
    /// Decoded value.
    value: String,
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    /// Raw (still escaped) character data.
    Text(String),
    /// Comment, processing instruction, CDATA section or DOCTYPE, verbatim.
    Raw(String),
    Elem(Elem),
}

#[derive(Debug, Clone, PartialEq)]
struct Elem {
    qname: String,
    attrs: Vec<Attr>,
    children: Vec<Node>,
    /// Written as `<a/>` (only while it has no children).
    self_closing: bool,
}

type Scope = Vec<(String, String)>;

fn split_q(q: &str) -> (&str, &str) {
    match q.split_once(':') {
        Some((p, l)) => (p, l),
        None => ("", q),
    }
}

fn scope_with(parent: &Scope, e: &Elem) -> Scope {
    let mut s = parent.clone();
    for a in &e.attrs {
        if a.qname == "xmlns" {
            s.push((String::new(), a.value.clone()));
        } else if let Some(p) = a.qname.strip_prefix("xmlns:") {
            s.push((p.to_string(), a.value.clone()));
        }
    }
    s
}

fn lookup<'a>(scope: &'a Scope, prefix: &str) -> Option<&'a str> {
    if prefix == "xml" {
        return Some(NS_XML);
    }
    scope
        .iter()
        .rev()
        .find(|(p, _)| p == prefix)
        .map(|(_, u)| u.as_str())
}

/// Namespace URI and local name of an element name (default namespace applies).
fn elem_name(scope: &Scope, qname: &str) -> (Option<String>, String) {
    let (p, l) = split_q(qname);
    (lookup(scope, p).map(str::to_string), l.to_string())
}

/// Attributes without a prefix have no namespace.
fn attr_name(scope: &Scope, qname: &str) -> (Option<String>, String) {
    let (p, l) = split_q(qname);
    if p.is_empty() {
        return (None, l.to_string());
    }
    (lookup(scope, p).map(str::to_string), l.to_string())
}

// ------------------------------------------------------------------ escaping

pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|e| *e <= 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let rep = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix('#')
                .and_then(|n| {
                    if let Some(h) = n.strip_prefix('x').or_else(|| n.strip_prefix('X')) {
                        u32::from_str_radix(h, 16).ok()
                    } else {
                        n.parse::<u32>().ok()
                    }
                })
                .and_then(char::from_u32),
        };
        match rep {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn escape_text(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' && c != '\r' => {}
            c => o.push(c),
        }
    }
    o
}

fn escape_attr(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '"' => o.push_str("&quot;"),
            '\n' => o.push_str("&#xA;"),
            '\r' => o.push_str("&#xD;"),
            '\t' => o.push_str("&#x9;"),
            c if (c as u32) < 0x20 => {}
            c => o.push(c),
        }
    }
    o
}

// ------------------------------------------------------------------ parsing

struct Parser<'a> {
    s: &'a str,
    pos: usize,
}

const MAX_DEPTH: usize = 64;

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.pos..]
    }

    fn starts(&self, p: &str) -> bool {
        self.rest().starts_with(p)
    }

    fn skip_ws(&mut self) {
        let r = self.rest();
        self.pos += r.len() - r.trim_start().len();
    }

    fn name(&mut self) -> Res<&'a str> {
        let r = self.rest();
        let end = r
            .find(|c: char| c.is_whitespace() || matches!(c, '=' | '>' | '/' | '<' | '"' | '\''))
            .unwrap_or(r.len());
        if end == 0 {
            return err(format!("expected a name at byte {}", self.pos));
        }
        self.pos += end;
        Ok(&r[..end])
    }

    /// Parses nodes until the closing tag of the parent (consumed) or the end of input.
    fn nodes(&mut self, depth: usize, parent: Option<&str>) -> Res<Vec<Node>> {
        if depth > MAX_DEPTH {
            return err("elements are nested too deeply");
        }
        let mut out = Vec::new();
        loop {
            if self.pos >= self.s.len() {
                return match parent {
                    None => Ok(out),
                    Some(p) => err(format!("unclosed element <{p}>")),
                };
            }
            if self.starts("<") {
                if self.starts("<!--") {
                    let end = self
                        .rest()
                        .find("-->")
                        .ok_or_else(|| XmpError("unterminated comment".into()))?;
                    out.push(Node::Raw(self.rest()[..end + 3].to_string()));
                    self.pos += end + 3;
                } else if self.starts("<![CDATA[") {
                    let end = self
                        .rest()
                        .find("]]>")
                        .ok_or_else(|| XmpError("unterminated CDATA".into()))?;
                    out.push(Node::Raw(self.rest()[..end + 3].to_string()));
                    self.pos += end + 3;
                } else if self.starts("<?") {
                    let end = self
                        .rest()
                        .find("?>")
                        .ok_or_else(|| XmpError("unterminated processing instruction".into()))?;
                    out.push(Node::Raw(self.rest()[..end + 2].to_string()));
                    self.pos += end + 2;
                } else if self.starts("<!") {
                    // DOCTYPE (possibly with an internal subset)
                    let r = self.rest();
                    let mut depth_br = 0i32;
                    let mut end = None;
                    for (i, c) in r.char_indices() {
                        match c {
                            '[' => depth_br += 1,
                            ']' => depth_br -= 1,
                            '>' if depth_br <= 0 => {
                                end = Some(i);
                                break;
                            }
                            _ => {}
                        }
                    }
                    let end = end.ok_or_else(|| XmpError("unterminated declaration".into()))?;
                    out.push(Node::Raw(r[..end + 1].to_string()));
                    self.pos += end + 1;
                } else if self.starts("</") {
                    self.pos += 2;
                    let name = self.name()?;
                    self.skip_ws();
                    if !self.starts(">") {
                        return err("malformed closing tag");
                    }
                    self.pos += 1;
                    return match parent {
                        Some(p) if p == name => Ok(out),
                        Some(p) => err(format!("</{name}> closes <{p}>")),
                        None => err(format!("stray closing tag </{name}>")),
                    };
                } else {
                    self.pos += 1;
                    let qname = self.name()?.to_string();
                    let mut attrs = Vec::new();
                    loop {
                        self.skip_ws();
                        if self.starts("/>") {
                            self.pos += 2;
                            out.push(Node::Elem(Elem {
                                qname,
                                attrs,
                                children: Vec::new(),
                                self_closing: true,
                            }));
                            break;
                        }
                        if self.starts(">") {
                            self.pos += 1;
                            let children = self.nodes(depth + 1, Some(&qname))?;
                            out.push(Node::Elem(Elem {
                                qname,
                                attrs,
                                children,
                                self_closing: false,
                            }));
                            break;
                        }
                        if self.pos >= self.s.len() {
                            return err("unterminated tag");
                        }
                        let an = self.name()?.to_string();
                        self.skip_ws();
                        if !self.starts("=") {
                            return err(format!("attribute {an} has no value"));
                        }
                        self.pos += 1;
                        self.skip_ws();
                        let q = self.rest().chars().next();
                        let Some(q @ ('"' | '\'')) = q else {
                            return err(format!("attribute {an} is not quoted"));
                        };
                        self.pos += 1;
                        let end = self
                            .rest()
                            .find(q)
                            .ok_or_else(|| XmpError("unterminated attribute value".into()))?;
                        let raw = &self.rest()[..end];
                        self.pos += end + 1;
                        attrs.push(Attr {
                            qname: an,
                            value: decode_entities(raw),
                        });
                    }
                }
            } else {
                let end = self.rest().find('<').unwrap_or(self.rest().len());
                out.push(Node::Text(self.rest()[..end].to_string()));
                self.pos += end;
            }
        }
    }
}

// ------------------------------------------------------------------ serialising

fn write_nodes(nodes: &[Node], out: &mut String) {
    for n in nodes {
        match n {
            Node::Text(t) | Node::Raw(t) => out.push_str(t),
            Node::Elem(e) => {
                out.push('<');
                out.push_str(&e.qname);
                for a in &e.attrs {
                    out.push(' ');
                    out.push_str(&a.qname);
                    out.push_str("=\"");
                    out.push_str(&escape_attr(&a.value));
                    out.push('"');
                }
                if e.self_closing && e.children.is_empty() {
                    out.push_str("/>");
                } else {
                    out.push('>');
                    write_nodes(&e.children, out);
                    out.push_str("</");
                    out.push_str(&e.qname);
                    out.push('>');
                }
            }
        }
    }
}

// ------------------------------------------------------------------ document

/// Location of an `rdf:Description`: child indices from the document root, plus its scope.
struct Desc {
    path: Vec<usize>,
    scope: Scope,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Xmp {
    nodes: Vec<Node>,
}

fn elem_at<'a>(nodes: &'a [Node], path: &[usize]) -> &'a Elem {
    let mut cur = nodes;
    let mut last = None;
    for i in path {
        match &cur[*i] {
            Node::Elem(e) => {
                last = Some(e);
                cur = &e.children;
            }
            _ => unreachable!("path points at an element"),
        }
    }
    last.expect("non-empty path")
}

fn elem_at_mut<'a>(nodes: &'a mut [Node], path: &[usize]) -> &'a mut Elem {
    let (first, rest) = path.split_first().expect("non-empty path");
    match &mut nodes[*first] {
        Node::Elem(e) => {
            if rest.is_empty() {
                e
            } else {
                elem_at_mut(&mut e.children, rest)
            }
        }
        _ => unreachable!("path points at an element"),
    }
}

/// Text of an element: decoded character data and CDATA, trimmed.
fn text_of(e: &Elem) -> String {
    let mut s = String::new();
    for c in &e.children {
        match c {
            Node::Text(t) => s.push_str(&decode_entities(t)),
            Node::Raw(r) if r.starts_with("<![CDATA[") => {
                s.push_str(&r[9..r.len().saturating_sub(3)]);
            }
            _ => {}
        }
    }
    s.trim().to_string()
}

fn child_elems(e: &Elem) -> impl Iterator<Item = &Elem> {
    e.children.iter().filter_map(|c| match c {
        Node::Elem(e) => Some(e),
        _ => None,
    })
}

impl Xmp {
    pub fn parse(text: &str) -> Res<Xmp> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut p = Parser { s: text, pos: 0 };
        let nodes = p.nodes(0, None)?;
        let x = Xmp { nodes };
        if x.root_elem_count() == 0 {
            return err("no XML element");
        }
        Ok(x)
    }

    /// A fresh packet with one empty `rdf:Description`.
    pub fn empty() -> Xmp {
        Xmp::parse(EMPTY_PACKET).expect("template parses")
    }

    fn root_elem_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| matches!(n, Node::Elem(_)))
            .count()
    }

    pub fn serialize(&self) -> String {
        let mut out = String::new();
        write_nodes(&self.nodes, &mut out);
        out
    }

    fn descriptions(&self) -> Vec<Desc> {
        fn walk(
            nodes: &[Node],
            path: &mut Vec<usize>,
            scope: &Scope,
            out: &mut Vec<Desc>,
            depth: usize,
        ) {
            for (i, n) in nodes.iter().enumerate() {
                let Node::Elem(e) = n else { continue };
                let sc = scope_with(scope, e);
                path.push(i);
                let (ns, local) = elem_name(&sc, &e.qname);
                if ns.as_deref() == Some(NS_RDF) && local == "Description" {
                    out.push(Desc {
                        path: path.clone(),
                        scope: sc,
                    });
                } else if depth < 6 {
                    walk(&e.children, path, &sc, out, depth + 1);
                }
                path.pop();
            }
        }
        let mut out = Vec::new();
        walk(&self.nodes, &mut Vec::new(), &Vec::new(), &mut out, 0);
        out
    }

    // ---- reading

    /// Value of a simple property (attribute or element form) of the first Description that has it.
    pub fn simple(&self, ns: &str, local: &str) -> Option<String> {
        for d in self.descriptions() {
            let e = elem_at(&self.nodes, &d.path);
            for a in &e.attrs {
                if a.qname.starts_with("xmlns") {
                    continue;
                }
                let (ans, al) = attr_name(&d.scope, &a.qname);
                if ans.as_deref() == Some(ns) && al == local {
                    return Some(a.value.trim().to_string());
                }
            }
            for c in child_elems(e) {
                let sc = scope_with(&d.scope, c);
                let (cns, cl) = elem_name(&sc, &c.qname);
                if cns.as_deref() == Some(ns) && cl == local {
                    // `<xmp:Rating><rdf:Description .../></xmp:Rating>` style wrappers are not supported
                    return Some(text_of(c));
                }
            }
        }
        None
    }

    /// Items of an array property (`rdf:Bag` / `rdf:Seq` / `rdf:Alt`).
    pub fn array(&self, ns: &str, local: &str) -> Vec<String> {
        for d in self.descriptions() {
            let e = elem_at(&self.nodes, &d.path);
            for c in child_elems(e) {
                let sc = scope_with(&d.scope, c);
                let (cns, cl) = elem_name(&sc, &c.qname);
                if cns.as_deref() != Some(ns) || cl != local {
                    continue;
                }
                let mut items = Vec::new();
                for container in child_elems(c) {
                    let csc = scope_with(&sc, container);
                    for li in child_elems(container) {
                        let lsc = scope_with(&csc, li);
                        let (lns, ll) = elem_name(&lsc, &li.qname);
                        if lns.as_deref() == Some(NS_RDF) && ll == "li" {
                            let t = text_of(li);
                            if !t.is_empty() {
                                items.push(t);
                            }
                        }
                    }
                }
                return items;
            }
        }
        Vec::new()
    }

    pub fn rating(&self) -> Option<i64> {
        let v = self.simple(NS_XMP, "Rating")?;
        let f: f64 = v.parse().ok()?;
        f.is_finite().then(|| f.round() as i64)
    }

    pub fn label(&self) -> Option<String> {
        self.simple(NS_XMP, "Label").filter(|s| !s.is_empty())
    }

    /// `xmpDM:pick` (1 = flagged, -1 = rejected).
    pub fn pick(&self) -> Option<i64> {
        self.simple(NS_XMPDM, "pick")?.parse().ok()
    }

    pub fn keywords(&self) -> Vec<String> {
        self.array(NS_DC, "subject")
    }

    pub fn hierarchical_keywords(&self) -> Vec<String> {
        self.array(NS_LR, "hierarchicalSubject")
    }

    /// darktable's `darktable:colorlabels` (0 red, 1 yellow, 2 green, 3 blue, 4 purple).
    pub fn darktable_labels(&self) -> Vec<i64> {
        self.array(NS_DARKTABLE, "colorlabels")
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect()
    }

    // ---- writing

    fn ensure_description(&mut self) -> Res<()> {
        if !self.descriptions().is_empty() {
            return Ok(());
        }
        // add a Description to an existing rdf:RDF
        fn find_rdf(
            nodes: &[Node],
            path: &mut Vec<usize>,
            scope: &Scope,
            depth: usize,
        ) -> Option<Vec<usize>> {
            for (i, n) in nodes.iter().enumerate() {
                let Node::Elem(e) = n else { continue };
                let sc = scope_with(scope, e);
                path.push(i);
                let (ns, local) = elem_name(&sc, &e.qname);
                if ns.as_deref() == Some(NS_RDF) && local == "RDF" {
                    return Some(path.clone());
                }
                if depth < 3 {
                    if let Some(p) = find_rdf(&e.children, path, &sc, depth + 1) {
                        return Some(p);
                    }
                }
                path.pop();
            }
            None
        }
        let Some(path) = find_rdf(&self.nodes, &mut Vec::new(), &Vec::new(), 0) else {
            return err("no rdf:RDF element to add properties to");
        };
        // resolve the prefix of the RDF namespace at that element
        let mut scope = Scope::new();
        {
            let mut cur: &[Node] = &self.nodes;
            for i in &path {
                if let Node::Elem(e) = &cur[*i] {
                    scope = scope_with(&scope, e);
                    cur = &e.children;
                }
            }
        }
        let prefix = scope
            .iter()
            .rev()
            .find(|(_, u)| u == NS_RDF)
            .map(|(p, _)| p.clone())
            .unwrap_or_else(|| "rdf".into());
        let q = if prefix.is_empty() {
            "Description".to_string()
        } else {
            format!("{prefix}:Description")
        };
        let rdf = elem_at_mut(&mut self.nodes, &path);
        rdf.self_closing = false;
        rdf.children.push(Node::Text("\n  ".into()));
        rdf.children.push(Node::Elem(Elem {
            qname: q,
            attrs: vec![Attr {
                qname: format!("{}:about", if prefix.is_empty() { "rdf" } else { &prefix }),
                value: String::new(),
            }],
            children: Vec::new(),
            self_closing: true,
        }));
        rdf.children.push(Node::Text("\n ".into()));
        Ok(())
    }

    /// Prefix bound to `ns` in `scope`, declaring `std_prefix` (or a variant) on the Description
    /// when needed.
    fn prefix_for(&mut self, d: &Desc, ns: &str, std_prefix: &str) -> String {
        if let Some((p, _)) = d.scope.iter().rev().find(|(p, u)| u == ns && !p.is_empty()) {
            return p.clone();
        }
        let mut prefix = std_prefix.to_string();
        let mut n = 1;
        while lookup(&d.scope, &prefix).is_some() {
            prefix = format!("{std_prefix}{n}");
            n += 1;
        }
        let e = elem_at_mut(&mut self.nodes, &d.path);
        e.attrs.push(Attr {
            qname: format!("xmlns:{prefix}"),
            value: ns.to_string(),
        });
        prefix
    }

    /// Sets (or with `None` removes) a simple property, keeping its current form.
    pub fn set_simple(
        &mut self,
        ns: &str,
        std_prefix: &str,
        local: &str,
        value: Option<&str>,
    ) -> Res<()> {
        self.ensure_description()?;
        let descs = self.descriptions();
        for d in &descs {
            let e = elem_at_mut(&mut self.nodes, &d.path);
            if let Some(i) = e.attrs.iter().position(|a| {
                !a.qname.starts_with("xmlns") && {
                    let (ans, al) = attr_name(&d.scope, &a.qname);
                    ans.as_deref() == Some(ns) && al == local
                }
            }) {
                match value {
                    Some(v) => e.attrs[i].value = v.to_string(),
                    None => {
                        e.attrs.remove(i);
                    }
                }
                return Ok(());
            }
            if let Some(i) = e.children.iter().position(|c| match c {
                Node::Elem(c) => {
                    let sc = scope_with(&d.scope, c);
                    let (cns, cl) = elem_name(&sc, &c.qname);
                    cns.as_deref() == Some(ns) && cl == local
                }
                _ => false,
            }) {
                match value {
                    Some(v) => {
                        if let Node::Elem(c) = &mut e.children[i] {
                            c.children = vec![Node::Text(escape_text(v))];
                            c.self_closing = false;
                        }
                    }
                    None => {
                        e.children.remove(i);
                    }
                }
                return Ok(());
            }
        }
        let Some(v) = value else { return Ok(()) };
        let d = &descs[0];
        let prefix = self.prefix_for(d, ns, std_prefix);
        let e = elem_at_mut(&mut self.nodes, &d.path);
        e.attrs.push(Attr {
            qname: format!("{prefix}:{local}"),
            value: v.to_string(),
        });
        Ok(())
    }

    /// Replaces the items of an array property (empty removes it).
    pub fn set_array(
        &mut self,
        ns: &str,
        std_prefix: &str,
        local: &str,
        items: &[String],
    ) -> Res<()> {
        self.ensure_description()?;
        let descs = self.descriptions();
        let li_node = |rdf_prefix: &str, t: &str| {
            Node::Elem(Elem {
                qname: format!("{rdf_prefix}:li"),
                attrs: Vec::new(),
                children: vec![Node::Text(escape_text(t))],
                self_closing: false,
            })
        };
        for d in &descs {
            let rdf_prefix = d
                .scope
                .iter()
                .rev()
                .find(|(_, u)| u == NS_RDF)
                .map(|(p, _)| p.clone())
                .unwrap_or_else(|| "rdf".into());
            let e = elem_at_mut(&mut self.nodes, &d.path);
            // attribute form of an array property is invalid; drop it
            e.attrs.retain(|a| {
                a.qname.starts_with("xmlns") || {
                    let (ans, al) = attr_name(&d.scope, &a.qname);
                    !(ans.as_deref() == Some(ns) && al == local)
                }
            });
            let pos = e.children.iter().position(|c| match c {
                Node::Elem(c) => {
                    let sc = scope_with(&d.scope, c);
                    let (cns, cl) = elem_name(&sc, &c.qname);
                    cns.as_deref() == Some(ns) && cl == local
                }
                _ => false,
            });
            let Some(i) = pos else { continue };
            if items.is_empty() {
                e.children.remove(i);
                return Ok(());
            }
            if let Node::Elem(c) = &mut e.children[i] {
                let csc = scope_with(&d.scope, c);
                // keep the container (Bag/Seq/Alt) and its indentation; replace the items
                let cont_idx = c.children.iter().position(|n| matches!(n, Node::Elem(_)));
                match cont_idx {
                    Some(ci) => {
                        if let Node::Elem(cont) = &mut c.children[ci] {
                            let sc = scope_with(&csc, cont);
                            let rp = sc
                                .iter()
                                .rev()
                                .find(|(_, u)| u == NS_RDF)
                                .map(|(p, _)| p.clone())
                                .unwrap_or(rdf_prefix.clone());
                            let (cns, cl) = elem_name(&sc, &cont.qname);
                            let is_list = cns.as_deref() == Some(NS_RDF)
                                && matches!(cl.as_str(), "Bag" | "Seq" | "Alt");
                            if is_list {
                                cont.children = items
                                    .iter()
                                    .flat_map(|t| [Node::Text("\n     ".into()), li_node(&rp, t)])
                                    .chain([Node::Text("\n    ".into())])
                                    .collect();
                                cont.self_closing = false;
                                return Ok(());
                            }
                        }
                        c.children.clear();
                    }
                    None => c.children.clear(),
                }
                c.self_closing = false;
                c.children.push(Node::Elem(Elem {
                    qname: format!("{rdf_prefix}:Bag"),
                    attrs: Vec::new(),
                    children: items
                        .iter()
                        .flat_map(|t| [Node::Text("\n     ".into()), li_node(&rdf_prefix, t)])
                        .chain([Node::Text("\n    ".into())])
                        .collect(),
                    self_closing: false,
                }));
            }
            return Ok(());
        }
        if items.is_empty() {
            return Ok(());
        }
        let d = &descs[0];
        let prefix = self.prefix_for(d, ns, std_prefix);
        let rdf_prefix = d
            .scope
            .iter()
            .rev()
            .find(|(_, u)| u == NS_RDF)
            .map(|(p, _)| p.clone())
            .unwrap_or_else(|| "rdf".into());
        let e = elem_at_mut(&mut self.nodes, &d.path);
        e.self_closing = false;
        e.children.push(Node::Text("\n   ".into()));
        e.children.push(Node::Elem(Elem {
            qname: format!("{prefix}:{local}"),
            attrs: Vec::new(),
            children: vec![
                Node::Text("\n    ".into()),
                Node::Elem(Elem {
                    qname: format!("{rdf_prefix}:Bag"),
                    attrs: Vec::new(),
                    children: items
                        .iter()
                        .flat_map(|t| [Node::Text("\n     ".into()), li_node(&rdf_prefix, t)])
                        .chain([Node::Text("\n    ".into())])
                        .collect(),
                    self_closing: false,
                }),
                Node::Text("\n   ".into()),
            ],
            self_closing: false,
        }));
        e.children.push(Node::Text("\n  ".into()));
        Ok(())
    }

    pub fn set_rating(&mut self, v: Option<i64>) -> Res<()> {
        self.set_simple(NS_XMP, "xmp", "Rating", v.map(|r| r.to_string()).as_deref())
    }

    pub fn set_label(&mut self, v: Option<&str>) -> Res<()> {
        self.set_simple(NS_XMP, "xmp", "Label", v)
    }

    pub fn set_keywords(&mut self, items: &[String]) -> Res<()> {
        self.set_array(NS_DC, "dc", "subject", items)
    }

    pub fn set_hierarchical_keywords(&mut self, items: &[String]) -> Res<()> {
        self.set_array(NS_LR, "lr", "hierarchicalSubject", items)
    }

    /// Removes every property, in attribute or element form and at any depth (struct fields
    /// and array items included), whose namespace URI and local name satisfy `drop`. The RDF
    /// and `x:xmpmeta` syntax itself is never removed. Returns how many properties went.
    pub fn remove_properties(&mut self, drop: &dyn Fn(&str, &str) -> bool) -> usize {
        prune(&mut self.nodes, &Scope::new(), drop, 0)
    }
}

fn removable(ns: Option<&str>, local: &str, drop: &dyn Fn(&str, &str) -> bool) -> bool {
    match ns {
        Some(ns) if ns != NS_RDF && ns != NS_META && ns != NS_XML => drop(ns, local),
        _ => false,
    }
}

/// See [`Xmp::remove_properties`].
fn prune(
    nodes: &mut Vec<Node>,
    scope: &Scope,
    drop: &dyn Fn(&str, &str) -> bool,
    depth: usize,
) -> usize {
    if depth > MAX_DEPTH {
        return 0;
    }
    let mut removed = 0;
    let mut i = 0;
    while i < nodes.len() {
        let Node::Elem(e) = &mut nodes[i] else {
            i += 1;
            continue;
        };
        let sc = scope_with(scope, e);
        let (ns, local) = elem_name(&sc, &e.qname);
        if removable(ns.as_deref(), &local, drop) {
            nodes.remove(i);
            removed += 1;
            // and the indentation in front of it
            if i > 0 && matches!(&nodes[i - 1], Node::Text(t) if t.trim().is_empty()) {
                nodes.remove(i - 1);
                i -= 1;
            }
            continue;
        }
        let before = e.attrs.len();
        e.attrs.retain(|a| {
            a.qname.starts_with("xmlns") || {
                let (ans, al) = attr_name(&sc, &a.qname);
                !removable(ans.as_deref(), &al, drop)
            }
        });
        removed += before - e.attrs.len();
        removed += prune(&mut e.children, &sc, drop, depth + 1);
        i += 1;
    }
    removed
}

const EMPTY_PACKET: &str = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"imagePicker\">\n <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  <rdf:Description rdf:about=\"\"/>\n </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>";

#[cfg(test)]
mod tests {
    use super::*;

    const LR: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:lr="http://ns.adobe.com/lightroom/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    xmp:Rating="4"
    xmp:Label="Red"
    crs:Exposure2012="+0.35"
    crs:Temperature="5200">
   <dc:subject>
    <rdf:Bag>
     <rdf:li>kyoto</rdf:li>
     <rdf:li>temple &amp; garden</rdf:li>
    </rdf:Bag>
   </dc:subject>
   <lr:hierarchicalSubject>
    <rdf:Bag>
     <rdf:li>Places|Japan|Kyoto</rdf:li>
    </rdf:Bag>
   </lr:hierarchicalSubject>
   <crs:ToneCurvePV2012>
    <rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq>
   </crs:ToneCurvePV2012>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

    #[test]
    fn reads_lightroom_attribute_form() {
        let x = Xmp::parse(LR).unwrap();
        assert_eq!(x.rating(), Some(4));
        assert_eq!(x.label().as_deref(), Some("Red"));
        assert_eq!(x.keywords(), ["kyoto", "temple & garden"]);
        assert_eq!(x.hierarchical_keywords(), ["Places|Japan|Kyoto"]);
    }

    #[test]
    fn reads_element_form_and_other_prefixes() {
        let s = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
 <rdf:Description xmlns:a="http://ns.adobe.com/xap/1.0/" xmlns:d="http://purl.org/dc/elements/1.1/">
  <a:Rating>-1</a:Rating><a:Label>Blue</a:Label>
  <d:subject><rdf:Seq><rdf:li>one</rdf:li><rdf:li>two</rdf:li></rdf:Seq></d:subject>
 </rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let x = Xmp::parse(s).unwrap();
        assert_eq!(x.rating(), Some(-1));
        assert_eq!(x.label().as_deref(), Some("Blue"));
        assert_eq!(x.keywords(), ["one", "two"]);
    }

    #[test]
    fn darktable_style_sample() {
        let s = r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="XMP Core 4.4.0-Exiv2">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:exif="http://ns.adobe.com/exif/1.0/"
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/"
    xmlns:darktable="http://darktable.sf.net/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
   xmp:Rating="-1"
   darktable:xmp_version="5"
   darktable:raw_params="0"
   xmpMM:DerivedFrom="IMG_0001.CR2">
   <darktable:history>
    <rdf:Seq>
     <rdf:li darktable:num="0" darktable:operation="exposure" darktable:enabled="1" darktable:params="00000000"/>
    </rdf:Seq>
   </darktable:history>
   <darktable:colorlabels>
    <rdf:Seq>
     <rdf:li>2</rdf:li>
     <rdf:li>4</rdf:li>
    </rdf:Seq>
   </darktable:colorlabels>
   <dc:subject>
    <rdf:Bag>
     <rdf:li>holiday</rdf:li>
    </rdf:Bag>
   </dc:subject>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let x = Xmp::parse(s).unwrap();
        assert_eq!(x.rating(), Some(-1));
        assert_eq!(x.darktable_labels(), [2, 4]);
        assert_eq!(x.keywords(), ["holiday"]);
        // rewriting keeps the history untouched
        let mut y = x.clone();
        y.set_rating(Some(3)).unwrap();
        let out = y.serialize();
        assert!(out.contains("darktable:operation=\"exposure\""));
        assert!(out.contains("xmpMM:DerivedFrom=\"IMG_0001.CR2\""));
        assert_eq!(Xmp::parse(&out).unwrap().rating(), Some(3));
    }

    #[test]
    fn unchanged_documents_round_trip_exactly_in_structure() {
        let x = Xmp::parse(LR).unwrap();
        let again = Xmp::parse(&x.serialize()).unwrap();
        assert_eq!(x, again);
        // untouched content survives a rewrite
        let mut y = x.clone();
        y.set_rating(Some(2)).unwrap();
        y.set_label(Some("Green")).unwrap();
        y.set_keywords(&["a".to_string(), "b <c>".to_string()])
            .unwrap();
        let out = y.serialize();
        for keep in [
            "crs:Exposure2012=\"+0.35\"",
            "crs:Temperature=\"5200\"",
            "<rdf:li>255, 255</rdf:li>",
            "<?xpacket end=\"w\"?>",
            "Adobe XMP Core 7.0-c000",
            "<rdf:li>Places|Japan|Kyoto</rdf:li>",
        ] {
            assert!(out.contains(keep), "{keep} lost in\n{out}");
        }
        let z = Xmp::parse(&out).unwrap();
        assert_eq!(z.rating(), Some(2));
        assert_eq!(z.label().as_deref(), Some("Green"));
        assert_eq!(z.keywords(), ["a", "b <c>"]);
        assert!(!out.contains("temple"));
    }

    #[test]
    fn creates_properties_with_declared_namespaces() {
        let mut x = Xmp::empty();
        x.set_rating(Some(5)).unwrap();
        x.set_label(Some("Yellow")).unwrap();
        x.set_keywords(&["café".to_string()]).unwrap();
        x.set_hierarchical_keywords(&["café".to_string()]).unwrap();
        let out = x.serialize();
        assert!(out.contains("xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\""));
        assert!(out.contains("xmlns:dc="));
        let y = Xmp::parse(&out).unwrap();
        assert_eq!(y.rating(), Some(5));
        assert_eq!(y.label().as_deref(), Some("Yellow"));
        assert_eq!(y.keywords(), ["café"]);
        assert_eq!(y.hierarchical_keywords(), ["café"]);
        // removal
        let mut z = y.clone();
        z.set_rating(None).unwrap();
        z.set_label(None).unwrap();
        z.set_keywords(&[]).unwrap();
        assert_eq!(z.rating(), None);
        assert_eq!(z.label(), None);
        assert!(z.keywords().is_empty());
    }

    #[test]
    fn element_form_is_updated_in_place() {
        let s = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:xmp="http://ns.adobe.com/xap/1.0/"><xmp:Rating>1</xmp:Rating></rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let mut x = Xmp::parse(s).unwrap();
        x.set_rating(Some(-1)).unwrap();
        let out = x.serialize();
        assert!(out.contains("<xmp:Rating>-1</xmp:Rating>"), "{out}");
    }

    #[test]
    fn rejects_garbage_but_handles_odd_input() {
        assert!(Xmp::parse("").is_err());
        assert!(Xmp::parse("not xml at all").is_err());
        assert!(Xmp::parse("<a><b></a>").is_err());
        assert!(Xmp::parse("<a x=1/>").is_err());
        assert!(Xmp::parse("<a>").is_err());
        let deep = "<a>".repeat(200);
        assert!(Xmp::parse(&deep).is_err());
        // comments, CDATA and DOCTYPE are carried along
        let s = "<!DOCTYPE foo [<!ENTITY x \"y\">]><!-- hi --><r><![CDATA[<raw>]]></r>";
        let x = Xmp::parse(s).unwrap();
        assert_eq!(x.serialize(), s);
        // a document without any Description cannot take properties
        let mut y = Xmp::parse("<a/>").unwrap();
        assert!(y.set_rating(Some(1)).is_err());
        // BOM
        assert!(Xmp::parse("\u{feff}<a/>").is_ok());
    }

    #[test]
    fn properties_are_removed_in_every_form_and_depth() {
        let s = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
 <rdf:Description rdf:about="" xmlns:exif="http://ns.adobe.com/exif/1.0/" xmlns:tiff="http://ns.adobe.com/tiff/1.0/" xmlns:ext="http://iptc.org/std/Iptc4xmpExt/2008-02-29/" exif:GPSLatitude="48,51.5N" tiff:Make="ACME">
  <exif:GPSLongitude>2,21.1E</exif:GPSLongitude>
  <ext:LocationShown><rdf:Bag><rdf:li rdf:parseType="Resource"><ext:City>Paris</ext:City><exif:GPSAltitude>35/1</exif:GPSAltitude></rdf:li></rdf:Bag></ext:LocationShown>
 </rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let mut x = Xmp::parse(s).unwrap();
        let n = x.remove_properties(&|ns, local| ns == NS_EXIF && local.starts_with("GPS"));
        assert_eq!(n, 3);
        let out = x.serialize();
        assert!(!out.contains("GPS"), "{out}");
        assert!(out.contains("<ext:City>Paris</ext:City>"), "{out}");
        let y = Xmp::parse(&out).unwrap();
        assert_eq!(y.simple(NS_TIFF, "Make").as_deref(), Some("ACME"));
        // the RDF syntax stays, whatever the predicate says
        let mut z = y.clone();
        z.remove_properties(&|_, _| true);
        let out = z.serialize();
        assert!(out.contains("<rdf:Description rdf:about=\"\""), "{out}");
        assert!(!out.contains("ACME") && !out.contains("Paris"), "{out}");
        assert!(Xmp::parse(&out).is_ok());
    }

    #[test]
    fn entities() {
        assert_eq!(
            decode_entities("a&amp;b&#65;&#x42;&lt;&bogus;&"),
            "a&bAB<&bogus;&"
        );
        assert_eq!(escape_text("a<b&c>"), "a&lt;b&amp;c&gt;");
    }
}
