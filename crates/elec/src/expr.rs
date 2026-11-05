//! The `<r:…>` label language used on drawings, list columns and reports.
//!
//! Every label in the studied applications is a template of literal text plus
//! `<r:path>` expressions, for example:
//!
//! ```text
//! (<r:Sheet>-<r:Row>-<r:Column>)
//! <r:Parent.WorkObject.GetWorkObjects(Class:Fuse).Current>
//! <r:Parent.WorkObject.DisplayedLabel(ExtendedLabel:True)>
//! <r:If(Test:Parent.PinOverlap(Direction:Up)=TRUE;True:False;False:True)>
//! <r:Parent.GetAssociatedWires.Number/Color>
//! ```
//!
//! This module parses that grammar into an AST and renders it against a resolver the
//! host supplies. It deliberately knows nothing about documents: the host decides what
//! `Parent` or `GetWorkObjects(Class:Fuse)` returns, which keeps the language reusable
//! for labels, list blocks and report exports.

/// A parsed template: literal runs with embedded references.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Template {
    pub parts: Vec<Part>,
}

/// One piece of a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Literal(String),
    Ref(Ref),
    /// `<#>` — the item index in a repeated context (list rows, columns).
    Index,
    /// `<r:If(...)>` — conditional text.
    If(Conditional),
}

/// A path expression: a root object followed by property and method steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub steps: Vec<Step>,
    /// `/`-joined alternates: `Number/Color` renders both, joined by the separator.
    pub join: Option<Join>,
}

/// A property access or a method call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Prop(String),
    Call(String, Vec<(String, Value)>),
}

/// The value of a named method argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A bare word: `Class:Fuse`, `Level:2`, `TRUE`.
    Word(String),
    /// A nested path.
    Ref(Ref),
}

/// `/`-separated property alternates and the text between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Join {
    pub props: Vec<String>,
    pub separator: String,
}

/// `If(Test:<ref>=<text>;True:<branch>;False:<branch>)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conditional {
    pub test: Ref,
    pub compare: Compare,
    pub expected: String,
    pub on_true: Branch,
    pub on_false: Branch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compare {
    Equal,
    NotEqual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Branch {
    Ref(Ref),
    Text(String),
}

/// Supplies values for references. The host implements this over its document.
pub trait Resolve {
    /// Value of a reference, or `None` when the path does not resolve.
    fn value(&self, path: &Ref) -> Option<String>;

    /// Zero-based index of the item being rendered, for `<#>`.
    fn index(&self) -> Option<u32> {
        None
    }
}

impl Template {
    /// Parse a template string. Unclosed `<r:` sequences are treated as literal text,
    /// so a partially typed label never fails to render.
    pub fn parse(input: &str) -> Self {
        let chars: Vec<char> = input.chars().collect();
        let mut parts = Vec::new();
        let mut literal = String::new();
        let mut i = 0usize;

        while i < chars.len() {
            // `<#>` is the bare index placeholder, written without the `r:` prefix.
            if chars[i] == '<' && chars.get(i + 1) == Some(&'#') && chars.get(i + 2) == Some(&'>')
            {
                if !literal.is_empty() {
                    parts.push(Part::Literal(std::mem::take(&mut literal)));
                }
                parts.push(Part::Index);
                i += 3;
                continue;
            }
            if chars[i] == '<' && chars.get(i + 1) == Some(&'r') {
                match find_terminator(&chars, i + 2) {
                    Some(end) => {
                        let inner: String = chars[i + 2..end].iter().collect();
                        let inner = inner.strip_prefix(':').unwrap_or(&inner).to_string();
                        if !literal.is_empty() {
                            parts.push(Part::Literal(std::mem::take(&mut literal)));
                        }
                        parts.push(parse_body(&inner));
                        i = end + 1;
                    }
                    None => {
                        literal.push(chars[i]);
                        i += 1;
                    }
                }
                continue;
            }
            literal.push(chars[i]);
            i += 1;
        }
        if !literal.is_empty() {
            parts.push(Part::Literal(literal));
        }
        Self { parts }
    }

    /// Render against a resolver. Unresolved references render as an empty string.
    pub fn render(&self, ctx: &dyn Resolve) -> String {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                Part::Literal(text) => out.push_str(text),
                Part::Ref(reference) => {
                    if let Some(v) = ctx.value(reference) {
                        out.push_str(&v);
                    }
                }
                Part::Index => {
                    if let Some(i) = ctx.index() {
                        out.push_str(&(i + 1).to_string());
                    }
                }
                Part::If(cond) => out.push_str(&render_conditional(cond, ctx)),
            }
        }
        out
    }

    /// True when the template is only literal text — useful to skip evaluation.
    pub fn is_literal(&self) -> bool {
        self.parts.iter().all(|p| matches!(p, Part::Literal(_)))
    }
}

fn render_conditional(cond: &Conditional, ctx: &dyn Resolve) -> String {
    let actual = ctx.value(&cond.test).unwrap_or_default();
    let equal = actual.trim().eq_ignore_ascii_case(cond.expected.trim());
    let take_true = match cond.compare {
        Compare::Equal => equal,
        Compare::NotEqual => !equal,
    };
    let branch = if take_true { &cond.on_true } else { &cond.on_false };
    match branch {
        Branch::Text(text) => text.clone(),
        Branch::Ref(reference) => ctx.value(reference).unwrap_or_default(),
    }
}

/// Index of the `>` closing a `<r:` body, ignoring `>` inside parentheses so that the
/// `<>` operator in an `If` test does not terminate the template early.
fn find_terminator(chars: &[char], from: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = from;
    while i < chars.len() {
        match chars[i] {
            '(' => depth += 1,
            ')' => depth -= 1,
            '>' if depth <= 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

fn parse_body(body: &str) -> Part {
    if body.trim() == "#" {
        return Part::Index;
    }
    let mut p = Parser::new(body);
    p.skip_ws();
    if p.keyword("If") {
        if let Some(cond) = p.parse_conditional() {
            return Part::If(cond);
        }
        // A malformed `If(` falls back to text rather than losing the label.
        return Part::Literal(format!("<r:{}>", body));
    }
    match p.parse_ref() {
        Some(reference) => Part::Ref(reference),
        None => Part::Literal(format!("<r:{}>", body)),
    }
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn keyword(&mut self, word: &str) -> bool {
        let start = self.pos;
        for expected in word.chars() {
            if self.peek() != Some(expected) {
                self.pos = start;
                return false;
            }
            self.pos += 1;
        }
        // Only accept the keyword when it is not part of a longer identifier.
        if matches!(self.peek(), Some(c) if c.is_alphanumeric() || c == '_') {
            self.pos = start;
            return false;
        }
        true
    }

    fn ident(&mut self) -> Option<String> {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_alphanumeric() || c == '_') {
            self.pos += 1;
        }
        if self.pos == start {
            None
        } else {
            Some(self.chars[start..self.pos].iter().collect())
        }
    }

    /// `Root.Step(args).Step/Alternate`
    fn parse_ref(&mut self) -> Option<Ref> {
        let mut steps = Vec::new();
        let first = self.ident()?;
        steps.push(Step::Prop(first));
        // The first segment may itself be a call: `GetEnvironment.GetIconList`.
        if self.eat('(') {
            let name = match steps.pop() {
                Some(Step::Prop(name)) => name,
                _ => return None,
            };
            let args = self.parse_args()?;
            steps.push(Step::Call(name, args));
        }
        while self.eat('.') {
            let name = self.ident()?;
            if self.eat('(') {
                let args = self.parse_args()?;
                steps.push(Step::Call(name, args));
            } else {
                steps.push(Step::Prop(name));
            }
        }
        let join = if self.eat('/') {
            let mut props = Vec::new();
            loop {
                props.push(self.ident()?);
                if !self.eat('/') {
                    break;
                }
            }
            Some(Join {
                props,
                separator: "/".to_string(),
            })
        } else {
            None
        };
        Some(Ref { steps, join })
    }

    /// Arguments after `(` — `Key:Value` pairs separated by `;`. The closing `)` is
    /// consumed.
    fn parse_args(&mut self) -> Option<Vec<(String, Value)>> {
        let mut args = Vec::new();
        loop {
            self.skip_ws();
            if self.eat(')') {
                return Some(args);
            }
            if self.eat(';') {
                continue;
            }
            let key = self.ident()?;
            if !self.eat(':') {
                return None;
            }
            self.skip_ws();
            let value = self.parse_value()?;
            args.push((key, value));
            self.skip_ws();
            if self.eat(')') {
                return Some(args);
            }
            if !self.eat(';') {
                return None;
            }
        }
    }

    /// A bare word, or a nested path when the value continues past a single segment.
    fn parse_value(&mut self) -> Option<Value> {
        let start = self.pos;
        let first = self.ident()?;
        if self.peek() == Some('.') {
            self.pos = start;
            return self.parse_ref().map(Value::Ref);
        }
        let mut text = first;
        // Numbers and words such as `-K1` are common argument values.
        while matches!(self.peek(), Some(c) if c == '-' || c == '+' || c == '.' || c.is_alphanumeric())
        {
            text.push(self.bump()?);
        }
        Some(Value::Word(text))
    }

    /// `If(Test:<ref>=<text>;True:<branch>;False:<branch>)`
    fn parse_conditional(&mut self) -> Option<Conditional> {
        self.skip_ws();
        if !self.eat('(') {
            return None;
        }
        let mut test: Option<(Ref, Compare, String)> = None;
        let mut on_true = None;
        let mut on_false = None;
        loop {
            self.skip_ws();
            if self.eat(')') {
                break;
            }
            let key = self.ident()?;
            if !self.eat(':') {
                return None;
            }
            let key_lower = key.to_ascii_lowercase();
            if key_lower == "test" {
                let reference = self.parse_ref()?;
                let compare = if self.eat('<') && self.eat('>') {
                    Compare::NotEqual
                } else if self.eat('=') {
                    Compare::Equal
                } else {
                    return None;
                };
                self.skip_ws();
                let expected = self.parse_display_word()?;
                test = Some((reference, compare, expected));
            } else {
                let branch = self.parse_branch()?;
                match key_lower.as_str() {
                    "true" => on_true = Some(branch),
                    "false" => on_false = Some(branch),
                    _ => {}
                }
            }
            self.skip_ws();
            if self.eat(')') {
                break;
            }
            if !self.eat(';') {
                return None;
            }
        }
        let (test, compare, expected) = test?;
        Some(Conditional {
            test,
            compare,
            expected,
            on_true: on_true?,
            on_false: on_false?,
        })
    }

    /// A bare word on the right of a comparison, e.g. `TRUE`.
    fn parse_display_word(&mut self) -> Option<String> {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_alphanumeric() || c == '_' || c == '-') {
            self.pos += 1;
        }
        if self.pos == start {
            return None;
        }
        Some(self.chars[start..self.pos].iter().collect())
    }

    fn parse_branch(&mut self) -> Option<Branch> {
        self.skip_ws();
        let start = self.pos;
        let first = self.ident()?;
        if self.peek() == Some('.') || self.peek() == Some('(') {
            self.pos = start;
            return self.parse_ref().map(Branch::Ref);
        }
        Some(Branch::Text(first))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A resolver backed by exact path strings, so tests state expectations directly.
    struct Mock {
        values: HashMap<String, String>,
        index: Option<u32>,
    }

    impl Mock {
        fn new(pairs: &[(&str, &str)]) -> Self {
            Self {
                values: pairs
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
                index: None,
            }
        }
    }

    impl Resolve for Mock {
        fn value(&self, path: &Ref) -> Option<String> {
            self.values.get(&path_to_string(path)).cloned()
        }
        fn index(&self) -> Option<u32> {
            self.index
        }
    }

    fn path_to_string(path: &Ref) -> String {
        let mut out = String::new();
        for (i, step) in path.steps.iter().enumerate() {
            if i > 0 {
                out.push('.');
            }
            match step {
                Step::Prop(name) => out.push_str(name),
                Step::Call(name, args) => {
                    out.push_str(name);
                    out.push('(');
                    for (j, (k, v)) in args.iter().enumerate() {
                        if j > 0 {
                            out.push(';');
                        }
                        out.push_str(k);
                        out.push(':');
                        match v {
                            Value::Word(w) => out.push_str(w),
                            Value::Ref(r) => out.push_str(&path_to_string(r)),
                        }
                    }
                    out.push(')');
                }
            }
        }
        if let Some(join) = &path.join {
            for prop in &join.props {
                out.push('/');
                out.push_str(prop);
            }
        }
        out
    }

    #[test]
    fn plain_literal_needs_no_resolution() {
        let t = Template::parse("Bill of Material");
        assert!(t.is_literal());
        assert_eq!(t.render(&Mock::new(&[])), "Bill of Material");
    }

    #[test]
    fn sheet_coordinate_label() {
        let t = Template::parse("(<r:Sheet>-<r:Row>-<r:Column>)");
        let ctx = Mock::new(&[("Sheet", "3"), ("Row", "B"), ("Column", "7")]);
        assert_eq!(t.render(&ctx), "(3-B-7)");
    }

    #[test]
    fn simple_property_path() {
        let t = Template::parse("<r:Parent.TerminalNumber>");
        let ctx = Mock::new(&[("Parent.TerminalNumber", "12")]);
        assert_eq!(t.render(&ctx), "12");
    }

    #[test]
    fn nested_navigation() {
        let t = Template::parse("<r:GetParent.GetParent.DisplayedLabel>");
        let ctx = Mock::new(&[("GetParent.GetParent.DisplayedLabel", "-K1")]);
        assert_eq!(t.render(&ctx), "-K1");
    }

    #[test]
    fn method_call_with_named_argument() {
        let t = Template::parse("<r:Parent.WorkObject.GetWorkObjects(Class:Fuse).Current>");
        let ctx = Mock::new(&[(
            "Parent.WorkObject.GetWorkObjects(Class:Fuse).Current",
            "4",
        )]);
        assert_eq!(t.render(&ctx), "4");
    }

    #[test]
    fn boolean_style_argument() {
        let t = Template::parse("<r:Parent.WorkObject.DisplayedLabel(ExtendedLabel:True)>");
        let ctx = Mock::new(&[(
            "Parent.WorkObject.DisplayedLabel(ExtendedLabel:True)",
            "-K1 Ext",
        )]);
        assert_eq!(t.render(&ctx), "-K1 Ext");
    }

    #[test]
    fn joined_alternates() {
        let t = Template::parse("<r:Parent.GetAssociatedWires.Number/Color>");
        let ctx = Mock::new(&[("Parent.GetAssociatedWires.Number/Color", "12/BK")]);
        assert_eq!(t.render(&ctx), "12/BK");
    }

    #[test]
    fn index_placeholder() {
        let t = Template::parse("<r:GetClass.GetRoot><#>");
        let mut ctx = Mock::new(&[("GetClass.GetRoot", "Component")]);
        ctx.index = Some(2);
        assert_eq!(t.render(&ctx), "Component3");
    }

    #[test]
    fn conditional_takes_true_branch() {
        let t = Template::parse(
            "<r:If(Test:Parent.PinOverlap(Direction:Up)=TRUE;True:False;False:True)>",
        );
        let ctx = Mock::new(&[("Parent.PinOverlap(Direction:Up)", "TRUE")]);
        assert_eq!(t.render(&ctx), "False");
    }

    #[test]
    fn conditional_takes_false_branch() {
        let t = Template::parse(
            "<r:If(Test:Parent.PinOverlap(Direction:Up)=FALSE;True:False;False:True)>",
        );
        let ctx = Mock::new(&[("Parent.PinOverlap(Direction:Up)", "TRUE")]);
        assert_eq!(t.render(&ctx), "True");
    }

    #[test]
    fn not_equal_operator_does_not_end_the_template() {
        let t = Template::parse(
            "<r:If(Test:GetParent.Count(Count:True)<>0;TRUE:TRUE;FALSE:FALSE)>",
        );
        let ctx = Mock::new(&[("GetParent.Count(Count:True)", "2")]);
        assert_eq!(t.render(&ctx), "TRUE");
    }

    #[test]
    fn comparison_is_case_insensitive() {
        let t = Template::parse("<r:If(Test:Parent.Gender=Jack;True:J;False:P)>");
        let ctx = Mock::new(&[("Parent.Gender", "jack")]);
        assert_eq!(t.render(&ctx), "J");
    }

    #[test]
    fn unresolved_reference_renders_empty() {
        let t = Template::parse("A<r:Missing.Thing>B");
        assert_eq!(t.render(&Mock::new(&[])), "AB");
    }

    #[test]
    fn unterminated_expression_stays_literal() {
        let t = Template::parse("x <r:Parent.Terminal Number");
        assert_eq!(t.render(&Mock::new(&[])), "x <r:Parent.Terminal Number");
    }

    #[test]
    fn mixed_literal_and_references() {
        let t = Template::parse("<r:Parent.WorkObject.DisplayedLabel> - <r:Parent.WorkObject.GetTerminalStrip.DisplayedLabel>");
        let ctx = Mock::new(&[
            ("Parent.WorkObject.DisplayedLabel", "X1"),
            ("Parent.WorkObject.GetTerminalStrip.DisplayedLabel", "-X1"),
        ]);
        assert_eq!(t.render(&ctx), "X1 - -X1");
    }

    #[test]
    fn real_world_template_parses_and_renders() {
        let source = "<r:GetParent.GetParent.DisplayedLabel> (<r:Sheet>-<r:Row>-<r:Column>)";
        let t = Template::parse(source);
        let ctx = Mock::new(&[
            ("GetParent.GetParent.DisplayedLabel", "MCC-1"),
            ("Sheet", "5"),
            ("Row", "C"),
            ("Column", "2"),
        ]);
        assert_eq!(t.render(&ctx), "MCC-1 (5-C-2)");
    }
}
