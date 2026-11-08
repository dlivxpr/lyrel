//! Source-preserving bridge for the public structural math model.
//! Opaque commands remain atoms; they are never fed through the lossy importer.
use ratex_gpui::editor::model::{Atom, Row};
use std::ops::Range;

#[derive(Clone)]
pub(super) struct Seed {
    pub(super) row: Row,
    raw: String,
    atoms: Vec<Piece>,
}
#[derive(Clone)]
struct Piece {
    atom: Atom,
    range: Range<usize>,
    children: Vec<(Range<usize>, Seed)>,
}

impl Seed {
    pub(super) fn parse(raw: &str) -> Self { Self::parse_depth(raw, 0) }
    fn parse_depth(raw: &str, depth: usize) -> Self {
        if depth >= 64 || raw.len() > 262_144 {
            let atom = Atom::Sym(raw.into());
            return Self { row: Row { atoms: vec![atom.clone()] }, raw: raw.into(), atoms: vec![Piece { atom, range: 0..raw.len(), children: Vec::new() }] };
        }
        let mut parser = Parser { raw, at: 0, depth };
        let mut atoms: Vec<Piece> = Vec::new();
        while parser.at < raw.len() {
            parser.space();
            if parser.at == raw.len() { break; }
            let start = parser.at;
            let (atom, mut children) = parser.atom();
            if parser.at == start { parser.bump(); }
            if let Atom::SupSub { sup, sub } = &atom {
                if let Some(Piece { atom: Atom::SupSub { sup: old_sup, sub: old_sub }, range, children: old_children }) = atoms.last_mut() {
                    if old_sup.is_none() && sup.is_some() || old_sub.is_none() && sub.is_some() {
                        if sup.is_some() { *old_sup = sup.clone(); old_children.append(&mut children); }
                        else { *old_sub = sub.clone(); children.append(old_children); *old_children = children; }
                        range.end = parser.at;
                        continue;
                    }
                }
            }
            atoms.push(Piece { atom, range: start..parser.at, children });
        }
        Self { row: Row { atoms: atoms.iter().map(|piece| piece.atom.clone()).collect() }, raw: raw.into(), atoms }
    }

    pub(super) fn serialize(&self, row: &Row) -> String { self.serialize_row(row, false) }
    pub(super) fn preview(&self, row: &Row) -> String { self.serialize_row(row, true) }
    pub(super) fn selection_latex(&self, row: &Row, range: Range<usize>) -> String {
        let selected = Row { atoms: row.atoms[range].to_vec() };
        self.new_row_text(&selected, false, None)
    }
    fn serialize_row(&self, row: &Row, placeholders: bool) -> String {
        if placeholders && row.is_empty() { return r"\square".into(); }
        if row == &self.row && (!placeholders || !row_has_empty(row)) { return self.raw.clone(); }
        let old = &self.atoms;
        let new = &row.atoms;
        // Unchanged prefix/suffix atoms retain their exact spelling and trivia.
        let prefix = old.iter().zip(new).take_while(|(a, b)| &a.atom == *b && (!placeholders || !atom_has_empty(b))).count();
        let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| &a.atom == *b && (!placeholders || !atom_has_empty(b))).count();
        let start = old.get(prefix).map_or(self.raw.trim_end().len(), |p| p.range.start);
        let end = if old.len() - suffix > prefix { old[old.len() - suffix - 1].range.end } else { start };
        let mut out = self.raw[..start].to_string();
        for (ix, atom) in new[prefix..new.len() - suffix].iter().enumerate() {
            let seed = old.get(prefix + ix).filter(|_| prefix + ix < old.len() - suffix);
            let text = seed.map_or_else(|| self.new_atom_text(atom, placeholders, None), |piece| self.atom_text(piece, atom, placeholders));
            // Only separate control words from a following ASCII letter.
            if needs_separator(&out, &text) { out.push(' '); }
            out.push_str(&text);
            if let Some(seed) = seed {
                if ix + 1 < new.len() - suffix - prefix && prefix + ix + 1 < old.len() - suffix {
                    out.push_str(&self.raw[seed.range.end..old[prefix + ix + 1].range.start]);
                }
            }
        }
        if needs_separator(&out, &self.raw[end..]) { out.push(' '); }
        out.push_str(&self.raw[end..]);
        out
    }

    fn atom_text(&self, seed: &Piece, atom: &Atom, placeholders: bool) -> String {
        if atom == &seed.atom && (!placeholders || !atom_has_empty(atom)) { return self.raw[seed.range.clone()].into(); }
        if let (Atom::Matrix { rows }, Atom::Matrix { rows: old }) = (atom, &seed.atom) {
            if rows.len() != old.len() || rows.iter().zip(old).any(|(a, b)| a.len() != b.len()) {
                if let (Some((first, _)), Some((last, _))) = (seed.children.first(), seed.children.last()) {
                    let body = rows.iter().map(|row| row.iter().map(|cell| {
                        seed.children.iter().find(|(_, child)| &child.row == cell).map_or_else(|| cell.to_latex(), |(_, child)| child.serialize_row(cell, placeholders))
                    }).collect::<Vec<_>>().join("&")).collect::<Vec<_>>().join(r"\\");
                    return format!("{}{}{}", &self.raw[seed.range.start..first.start], body, &self.raw[last.end..seed.range.end]);
                }
            }
        }
        let rows = child_rows(atom);
        let old_rows = child_rows(&seed.atom);
        if same_wrapper(atom, &seed.atom)
            && rows.len() == seed.children.len() && rows.len() == old_rows.len()
            && !rows.is_empty()
        {
            let mut text = self.raw[seed.range.clone()].to_string();
            let mut replacements: Vec<_> = seed.children.iter().zip(rows).map(|((range, child), row)| (range.clone(), child.serialize_row(row, placeholders))).collect();
            replacements.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
            for (range, replacement) in replacements {
                text.replace_range(range.start - seed.range.start..range.end - seed.range.start, &replacement);
            }
            text
        } else { self.new_atom_text(atom, placeholders, Some(seed)) }
    }
    fn original_row(&self, row: &Row, preferred: Option<&Piece>) -> Option<&str> {
        if row == &self.row { return Some(&self.raw); }
        if row.is_empty() || row.atoms.len() > self.atoms.len() { return None; }
        let matches = |at: usize| self.row.atoms[at..at + row.atoms.len()] == row.atoms[..];
        let at = preferred.and_then(|piece| self.atoms.iter().position(|old| old.range == piece.range)).filter(|at| *at + row.atoms.len() <= self.atoms.len() && matches(*at))
            .or_else(|| (0..=self.atoms.len() - row.atoms.len()).find(|at| matches(*at)));
        at.map(|at| &self.raw[self.atoms[at].range.start..self.atoms[at + row.atoms.len() - 1].range.end])
    }
    fn new_row_text(&self, row: &Row, placeholders: bool, preferred: Option<&Piece>) -> String {
        if placeholders && row.is_empty() { return r"\square".into(); }
        if let Some(raw) = self.original_row(row, preferred).filter(|_| !placeholders || !row_has_empty(row)) { return raw.into(); }
        let mut out = String::new();
        for atom in &row.atoms {
            let origin = preferred.filter(|piece| &piece.atom == atom).or_else(|| self.atoms.iter().find(|piece| &piece.atom == atom));
            let text = origin.map_or_else(|| self.new_atom_text(atom, placeholders, None), |piece| self.atom_text(piece, atom, placeholders));
            if needs_separator(&out, &text) { out.push(' '); }
            out.push_str(&text);
        }
        out
    }
    fn new_atom_text(&self, atom: &Atom, placeholders: bool, preferred: Option<&Piece>) -> String {
        let row = |row: &Row| self.new_row_text(row, placeholders, preferred);
        match atom {
            Atom::Frac { num, den } => format!(r"\frac{{{}}}{{{}}}", row(num), row(den)),
            Atom::Sqrt { radicand, index } => match index { Some(index) => format!(r"\sqrt[{}]{{{}}}", row(index), row(radicand)), None => format!(r"\sqrt{{{}}}", row(radicand)) },
            Atom::SupSub { sup, sub } => {
                let mut out = String::new();
                if let Some(sub) = sub { out.push_str(&format!("_{{{}}}", row(sub))); }
                if let Some(sup) = sup { out.push_str(&format!("^{{{}}}", row(sup))); }
                out
            }
            Atom::Accent { accent, base } => format!("{accent}{{{}}}", row(base)),
            Atom::Delim { open, body, close } => format!(r"\left{open} {} \right{close}", row(body)),
            Atom::Matrix { rows } => {
                let body = rows.iter().map(|cells| cells.iter().map(row).collect::<Vec<_>>().join(" & ")).collect::<Vec<_>>().join(r" \\ ");
                format!(r"\begin{{pmatrix}}{body}\end{{pmatrix}}")
            }
            Atom::Sym(_) => atom.to_latex(),
        }
    }
}

fn row_has_empty(row: &Row) -> bool { row.is_empty() || row.atoms.iter().any(atom_has_empty) }
fn atom_has_empty(atom: &Atom) -> bool {
    match atom {
        Atom::Frac { num, den } => row_has_empty(num) || row_has_empty(den),
        Atom::Sqrt { radicand, index } => row_has_empty(radicand) || index.as_ref().is_some_and(row_has_empty),
        Atom::SupSub { sup, sub } => sup.as_ref().is_some_and(row_has_empty) || sub.as_ref().is_some_and(row_has_empty),
        Atom::Delim { body, .. } | Atom::Accent { base: body, .. } => row_has_empty(body),
        Atom::Matrix { rows } => rows.iter().flatten().any(row_has_empty),
        Atom::Sym(_) => false,
    }
}
fn same_wrapper(a: &Atom, b: &Atom) -> bool {
    match (a, b) {
        (Atom::Delim { open: a, close: x, .. }, Atom::Delim { open: b, close: y, .. }) => a == b && x == y,
        (Atom::Accent { accent: a, .. }, Atom::Accent { accent: b, .. }) => a == b,
        (Atom::SupSub { sup: a, sub: x }, Atom::SupSub { sup: b, sub: y }) => a.is_some() == b.is_some() && x.is_some() == y.is_some(),
        _ => std::mem::discriminant(a) == std::mem::discriminant(b),
    }
}

fn needs_separator(before: &str, after: &str) -> bool {
    if !after.starts_with(|ch: char| ch.is_ascii_alphabetic()) { return false; }
    let letters = before.bytes().rev().take_while(u8::is_ascii_alphabetic).count();
    letters > 0 && before.as_bytes().get(before.len().saturating_sub(letters + 1)) == Some(&b'\\')
}

fn child_rows(atom: &Atom) -> Vec<&Row> {
    match atom {
        Atom::Frac { num, den } => vec![num, den],
        Atom::Sqrt { radicand, index } => index.iter().chain(std::iter::once(radicand)).collect(),
        Atom::SupSub { sup, sub } => sub.iter().chain(sup.iter()).collect(),
        Atom::Accent { base, .. } | Atom::Delim { body: base, .. } => vec![base],
        Atom::Matrix { rows } => rows.iter().flatten().collect(),
        Atom::Sym(_) => Vec::new(),
    }
}

struct Parser<'a> { raw: &'a str, at: usize, depth: usize }
impl Parser<'_> {
    fn bump(&mut self) -> char {
        let ch = self.raw[self.at..].chars().next().unwrap();
        self.at += ch.len_utf8(); ch
    }
    fn space(&mut self) { while self.raw[self.at..].starts_with(char::is_whitespace) { self.bump(); } }
    fn group(&mut self, open: char, close: char) -> Option<(Range<usize>, Seed)> {
        self.space();
        if !self.raw[self.at..].starts_with(open) { return None; }
        self.bump();
        let start = self.at;
        let mut depth = 1;
        while self.at < self.raw.len() {
            let at = self.at;
            match self.bump() {
                '\\' => { if self.at < self.raw.len() { self.bump(); } }
                ch if ch == open => depth += 1,
                ch if ch == close => {
                    depth -= 1;
                    if depth == 0 { return Some((start..at, Seed::parse_depth(&self.raw[start..at], self.depth + 1))); }
                }
                _ => {}
            }
        }
        None
    }
    fn argument(&mut self) -> Option<(Range<usize>, Seed)> {
        self.space();
        if self.raw[self.at..].starts_with('{') { return self.group('{', '}'); }
        if self.at == self.raw.len() { return None; }
        let start = self.at;
        self.depth += 1;
        self.atom();
        self.depth -= 1;
        Some((start..self.at, Seed::parse_depth(&self.raw[start..self.at], self.depth + 1)))
    }
    fn atom(&mut self) -> (Atom, Vec<(Range<usize>, Seed)>) {
        let start = self.at;
        if self.depth >= 64 { self.at = self.raw.len(); return (Atom::Sym(self.raw[start..].into()), Vec::new()); }
        let ch = self.bump();
        let mut children = Vec::new();
        let atom = match ch {
            '^' | '_' => {
                if let Some(child) = self.argument() {
                    let row = child.1.row.clone(); children.push(child);
                    Atom::SupSub { sup: (ch == '^').then_some(row.clone()), sub: (ch == '_').then_some(row) }
                } else { Atom::Sym(ch.to_string()) }
            }
            '\\' => {
                while self.raw[self.at..].starts_with(|c: char| c.is_ascii_alphabetic()) { self.bump(); }
                if self.at == start + 1 && self.at < self.raw.len() { self.bump(); }
                let command = &self.raw[start..self.at];
                match command {
                    "\\frac" | "\\dfrac" | "\\tfrac" => {
                        let num = self.argument(); let den = self.argument();
                        match (num, den) {
                            (Some(num), Some(den)) => {
                                let atom = Atom::Frac { num: num.1.row.clone(), den: den.1.row.clone() };
                                children.extend([num, den]); atom
                            }
                            _ => Atom::Sym(self.raw[start..self.at].into()),
                        }
                    }
                    "\\sqrt" => {
                        let index = self.group('[', ']');
                        match self.argument() {
                            Some(body) => {
                                let atom = Atom::Sqrt { radicand: body.1.row.clone(), index: index.as_ref().map(|(_, seed)| seed.row.clone()) };
                                children.extend(index); children.push(body); atom
                            }
                            None => Atom::Sym(self.raw[start..self.at].into()),
                        }
                    }
                    "\\hat" | "\\widehat" | "\\bar" | "\\vec" | "\\tilde" | "\\widetilde" | "\\dot" | "\\ddot" | "\\check" | "\\breve" => {
                        match self.argument() {
                            Some(body) => {
                                let atom = Atom::Accent { accent: command.into(), base: body.1.row.clone() };
                                children.push(body); atom
                            }
                            None => Atom::Sym(self.raw[start..self.at].into()),
                        }
                    }
                    "\\begin" => {
                        let env = self.group('{', '}');
                        if let Some((range, _)) = env {
                            let name = &self.raw[range];
                            let ending = format!("\\end{{{name}}}");
                            if let Some(offset) = self.raw[self.at..].find(&ending) {
                                let body_start = self.at; let body_end = self.at + offset;
                                self.at = body_end + ending.len();
                                if matches!(name, "matrix" | "pmatrix" | "bmatrix" | "Bmatrix" | "vmatrix" | "Vmatrix" | "cases" | "aligned") {
                                    let mut rows = vec![Vec::new()];
                                    let mut cell = body_start; let mut at = body_start; let mut depth = 0usize;
                                    while at < body_end {
                                        let c = self.raw[at..].chars().next().unwrap();
                                        let separator = depth == 0 && (c == '&' || self.raw[at..].starts_with("\\\\"));
                                        if separator {
                                            let seed = Seed::parse_depth(&self.raw[cell..at], self.depth + 1); rows.last_mut().unwrap().push(seed.row.clone());
                                            children.push((cell..at, seed));
                                            if c != '&' { rows.push(Vec::new()); at += 1; }
                                            at += 1; cell = at; continue;
                                        }
                                        if c == '{' { depth += 1; } else if c == '}' { depth = depth.saturating_sub(1); }
                                        if c == '\\' { at += 1; if at < body_end { at += self.raw[at..].chars().next().unwrap().len_utf8(); } }
                                        else { at += c.len_utf8(); }
                                    }
                                    let seed = Seed::parse_depth(&self.raw[cell..body_end], self.depth + 1); rows.last_mut().unwrap().push(seed.row.clone()); children.push((cell..body_end, seed));
                                    if rows.iter().all(|row| row.len() == rows[0].len()) { Atom::Matrix { rows } }
                                    else { children.clear(); Atom::Sym(self.raw[start..self.at].into()) }
                                } else { Atom::Sym(self.raw[start..self.at].into()) }
                            } else { Atom::Sym(self.raw[start..self.at].into()) }
                        } else { Atom::Sym(self.raw[start..self.at].into()) }
                    }
                    "\\left" => {
                        self.space(); let delimiter_start = self.at;
                        if self.at < self.raw.len() { self.atom(); }
                        let open = self.raw[delimiter_start..self.at].to_string();
                        let body_start = self.at;
                        let mut nested = 0usize; let mut end = None;
                        while self.at < self.raw.len() {
                            if self.raw[self.at..].starts_with("\\left") { nested += 1; self.at += 5; }
                            else if self.raw[self.at..].starts_with("\\right") {
                                if nested == 0 { end = Some(self.at); break; }
                                nested -= 1; self.at += 6;
                            } else { self.bump(); }
                        }
                        if let Some(end) = end {
                            let child = Seed::parse_depth(&self.raw[body_start..end], self.depth + 1); self.at += 6; self.space();
                            let close_start = self.at; if self.at < self.raw.len() { self.atom(); }
                            let close = self.raw[close_start..self.at].to_string();
                            let atom = Atom::Delim { open, body: child.row.clone(), close };
                            children.push((body_start..end, child)); atom
                        } else { Atom::Sym(self.raw[start..self.at].into()) }
                    }
                    _ => {
                        // Keep wrapper arguments and unknown commands intact. They remain
                        // selectable opaque atoms, with the explicit source action available.
                        loop {
                            let before = self.at;
                            if self.group('{', '}').is_none() { self.at = before; break; }
                        }
                        Atom::Sym(self.raw[start..self.at].into())
                    }
                }
            }
            '{' => {
                self.at = start; let _ = self.group('{', '}');
                Atom::Sym(self.raw[start..self.at].into())
            }
            _ => Atom::Sym(ch.to_string()),
        };
        (atom, children)
    }
}
