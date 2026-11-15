//! Small gettext-compatible translator.
//!
//! The Linux build used `gettext-rs`, whose build script needs a Unix shell
//! and a system libintl even when it is told not to link the system copy. That
//! makes the workspace impossible to build on Windows, so the two functions
//! the product actually needs — `gettext` and `ngettext` — are implemented
//! here over the same `.mo` files every translation tooling step already
//! produces.
//!
//! Scope is deliberately the GNU MO format, not the whole gettext API:
//!
//! * one domain (`ferramari`), because the product ships one catalog;
//! * UTF-8 catalogs, which is what `msgfmt` writes for the shipped `.po`
//!   files;
//! * plural selection through the catalog's own `Plural-Forms` expression,
//!   parsed once when the catalog loads. Hardcoding English/Russian/Japanese
//!   rules would make a new translation silently pluralise like English.
//!
//! A missing or unreadable catalog is not an error: every lookup falls back to
//! the source string, which keeps the application usable while a packager
//! fixes an install.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

/// Translation domain, matching the `.mo` file installed by the Makefile.
pub const DOMAIN: &str = "ferramari";

static STATE: OnceLock<RwLock<Translator>> = OnceLock::new();

fn state() -> &'static RwLock<Translator> {
    STATE.get_or_init(|| RwLock::new(Translator::default()))
}

#[derive(Default)]
struct Translator {
    localedir: PathBuf,
    catalog: Option<Catalog>,
    /// The language the loaded catalog was compiled for, for diagnostics.
    language: String,
}

impl Translator {
    fn load(&mut self, language: Option<String>) {
        self.catalog = None;
        self.language = language.clone().unwrap_or_default();
        if let Some(language) = language {
            self.catalog = Catalog::load(&self.localedir, &language);
            match &self.catalog {
                Some(catalog) => log::debug!(
                    "[i18n] Loaded {} messages for {language}",
                    catalog.messages.len()
                ),
                None => log::debug!("[i18n] No catalog for {language}; using source strings"),
            }
        }
    }
}

/// Point the translator at the directory that contains
/// `<language>/LC_MESSAGES/ferramari.mo` and load the current language.
///
/// Idempotent: the UI calls it once at startup and `reload` after a language
/// change, so the directory has to outlive the first call's environment.
pub fn init(localedir: impl Into<PathBuf>) {
    // Resolve the language before taking the write lock. An earlier helper
    // read the shared translator to find the directory, so calling it while
    // this thread held the write guard deadlocked on the first start.
    let localedir = localedir.into();
    let language = current_language_from(&localedir);
    let mut state = state().write().unwrap_or_else(|poison| poison.into_inner());
    state.localedir = localedir;
    state.load(language);
}

/// Re-read the catalog for the language currently selected by the
/// environment. Call this after setting or clearing `LANGUAGE`.
pub fn reload() {
    let mut state = state().write().unwrap_or_else(|poison| poison.into_inner());
    let localedir = state.localedir.clone();
    state.load(current_language_from(&localedir));
}

/// Translate a message. Unknown messages and an uninitialised translator both
/// return the input unchanged.
pub fn tr(text: &str) -> String {
    let state = state().read().unwrap_or_else(|poison| poison.into_inner());
    state
        .catalog
        .as_ref()
        .and_then(|catalog| catalog.messages.get(text))
        .and_then(|forms| forms.first())
        .cloned()
        .unwrap_or_else(|| text.to_string())
}

/// Translate a counted noun. Falls back to the English singular/plural pair
/// when the catalog has no entry.
pub fn tr_n(singular: &str, plural: &str, n: u32) -> String {
    let state = state().read().unwrap_or_else(|poison| poison.into_inner());
    let Some(forms) = state
        .catalog
        .as_ref()
        .and_then(|catalog| catalog.messages.get(singular))
    else {
        return if n == 1 { singular } else { plural }.to_string();
    };
    if forms.is_empty() {
        return if n == 1 { singular } else { plural }.to_string();
    }
    let index = state
        .catalog
        .as_ref()
        .map(|catalog| catalog.plural.index(n as i64))
        .unwrap_or(0)
        .min(forms.len() - 1);
    forms[index].clone()
}

/// The language a catalog was loaded for, or an empty string when the
/// environment did not select one. Exposed for `--doctor` and tests.
pub fn active_language() -> String {
    state()
        .read()
        .unwrap_or_else(|poison| poison.into_inner())
        .language
        .clone()
}

fn current_language_from(_localedir: &Path) -> Option<String> {
    // `LANGUAGE` is a colon-separated preference list; the desktop's own
    // locale variables are the fallback. An explicit in-app choice writes
    // `LANGUAGE`, which is why it wins.
    for variable in ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        let Ok(value) = std::env::var(variable) else {
            continue;
        };
        for candidate in value.split(':') {
            if let Some(language) = normalise_language(candidate) {
                return Some(language);
            }
        }
    }
    None
}

fn normalise_language(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("c") || value.eq_ignore_ascii_case("posix") {
        return None;
    }
    let value = value.split('.').next().unwrap_or(value);
    let value = value.split('@').next().unwrap_or(value);
    if value.is_empty() {
        return None;
    }
    let value = value.replace('-', "_");
    // `LANGUAGE=en_US:ru` must try en_US, then en. The caller only sees the
    // first candidate; `Catalog::load` walks the parent tags itself.
    Some(value)
}

struct Catalog {
    messages: HashMap<String, Vec<String>>,
    plural: PluralRule,
}

impl Catalog {
    fn load(localedir: &Path, language: &str) -> Option<Self> {
        let mut candidates = Vec::new();
        let mut tag = language.to_string();
        loop {
            candidates.push(tag.clone());
            let Some((parent, _)) = tag.rsplit_once('_') else {
                break;
            };
            if parent.is_empty() {
                break;
            }
            tag = parent.to_string();
        }
        for candidate in candidates {
            let path = localedir
                .join(&candidate)
                .join("LC_MESSAGES")
                .join(format!("{DOMAIN}.mo"));
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            match Self::parse(&bytes) {
                Some(catalog) => return Some(catalog),
                None => log::warn!("[i18n] {} is not a valid MO catalog", path.display()),
            }
        }
        None
    }

    fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 28 {
            return None;
        }
        let magic_le = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
        let (little_endian, count, originals, translations) = if magic_le == 0x9504_12de {
            (
                true,
                read_u32(bytes, 8, true)?,
                read_u32(bytes, 12, true)?,
                read_u32(bytes, 16, true)?,
            )
        } else if magic_le.swap_bytes() == 0x9504_12de {
            (
                false,
                read_u32(bytes, 8, false)?,
                read_u32(bytes, 12, false)?,
                read_u32(bytes, 16, false)?,
            )
        } else {
            return None;
        };
        // A malformed table must not make the loader index far outside the
        // file; 200 000 entries is already absurd for one UI catalog.
        if count > 200_000 {
            return None;
        }
        let mut messages = HashMap::new();
        let mut plural = PluralRule::default();
        for index in 0..count {
            let original = read_string(bytes, originals, index, little_endian)?;
            let translated = read_string(bytes, translations, index, little_endian)?;
            let original = String::from_utf8(original).ok()?;
            if original.is_empty() {
                // Header: plural rules and the catalog's language live here.
                if let Ok(header) = String::from_utf8(translated) {
                    plural = PluralRule::parse_header(&header);
                }
                continue;
            }
            // A plural entry's msgid is `singular\0plural`; `tr_n` looks the
            // message up by its singular form, so the key must be split at
            // the first NUL. Keeping the raw msgid silently disabled every
            // counted noun in a translated catalog.
            let singular = original.split('\0').next().unwrap_or(&original);
            let forms: Vec<String> = translated
                .split(|byte| *byte == 0)
                .filter_map(|form| String::from_utf8(form.to_vec()).ok())
                .collect();
            if !singular.is_empty() && !forms.is_empty() {
                messages.insert(singular.to_string(), forms);
            }
        }
        Some(Self { messages, plural })
    }
}

fn read_u32(bytes: &[u8], offset: u32, little_endian: bool) -> Option<u32> {
    let offset = offset as usize;
    let raw: [u8; 4] = bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
    Some(if little_endian {
        u32::from_le_bytes(raw)
    } else {
        u32::from_be_bytes(raw)
    })
}

fn read_string(bytes: &[u8], table: u32, index: u32, little_endian: bool) -> Option<Vec<u8>> {
    let entry = table.checked_add(index.checked_mul(8)?)?;
    let length = read_u32(bytes, entry, little_endian)? as usize;
    let offset = read_u32(bytes, entry.checked_add(4)?, little_endian)? as usize;
    bytes
        .get(offset..offset.checked_add(length)?)
        .map(Vec::from)
}

/// The catalog's `Plural-Forms` expression, parsed into an AST.
#[derive(Default)]
struct PluralRule {
    nplurals: u32,
    expression: Option<Expr>,
}

impl PluralRule {
    fn parse_header(header: &str) -> Self {
        let mut rule = Self::default();
        for line in header.lines() {
            let Some(value) = line.strip_prefix("Plural-Forms:") else {
                continue;
            };
            for part in value.split(';') {
                let part = part.trim();
                if let Some(value) = part.strip_prefix("nplurals=") {
                    rule.nplurals = value.trim().parse().unwrap_or(0);
                } else if let Some(value) = part.strip_prefix("plural=") {
                    rule.expression = Expr::parse(value.trim());
                }
            }
        }
        rule
    }

    fn index(&self, n: i64) -> usize {
        let Some(expression) = &self.expression else {
            return usize::from(n != 1);
        };
        let index = expression.eval(n).max(0) as usize;
        if self.nplurals > 0 {
            index.min(self.nplurals as usize - 1)
        } else {
            index
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    Number(i64),
    N,
    Not(Box<Expr>),
    Neg(Box<Expr>),
    Binary(Box<Expr>, Operator, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operator {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl Expr {
    fn parse(text: &str) -> Option<Self> {
        let mut parser = Parser {
            tokens: tokenize(text)?,
            position: 0,
        };
        let expression = parser.ternary()?;
        (parser.position == parser.tokens.len()).then_some(expression)
    }

    fn eval(&self, n: i64) -> i64 {
        match self {
            Expr::Number(value) => *value,
            Expr::N => n,
            Expr::Not(inner) => truth(inner.eval(n) == 0_i64),
            Expr::Neg(inner) => -inner.eval(n),
            Expr::Binary(left, operator, right) => {
                let left = left.eval(n);
                // Short-circuit so `n != 0 && 10 / n` cannot divide by zero.
                match operator {
                    Operator::And => {
                        return truth(left != 0 && right.eval(n) != 0);
                    }
                    Operator::Or => {
                        return truth(left != 0 || right.eval(n) != 0);
                    }
                    _ => {}
                }
                let right = right.eval(n);
                let value = match operator {
                    Operator::Eq => left == right,
                    Operator::Ne => left != right,
                    Operator::Lt => left < right,
                    Operator::Le => left <= right,
                    Operator::Gt => left > right,
                    Operator::Ge => left >= right,
                    Operator::Add => return left.saturating_add(right),
                    Operator::Sub => return left.saturating_sub(right),
                    Operator::Mul => return left.saturating_mul(right),
                    Operator::Div => {
                        return if right == 0 { 0 } else { left / right };
                    }
                    Operator::Rem => {
                        return if right == 0 { 0 } else { left % right };
                    }
                    Operator::And | Operator::Or => unreachable!(),
                };
                truth(value)
            }
            Expr::Ternary(condition, then, otherwise) => {
                if condition.eval(n) != 0 {
                    then.eval(n)
                } else {
                    otherwise.eval(n)
                }
            }
        }
    }
}

/// The integer a comparison contributes to a plural expression.
fn truth(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Number(i64),
    N,
    Operator(&'static str),
    Open,
    Close,
    Question,
    Colon,
}

fn tokenize(text: &str) -> Option<Vec<Token>> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if byte.is_ascii_digit() {
            let start = index;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            tokens.push(Token::Number(text[start..index].parse().ok()?));
            continue;
        }
        if byte == b'n' {
            tokens.push(Token::N);
            index += 1;
            continue;
        }
        let two = text.get(index..index + 2);
        if let Some(operator) = two.and_then(|two| match two {
            "||" => Some("||"),
            "&&" => Some("&&"),
            "==" => Some("=="),
            "!=" => Some("!="),
            "<=" => Some("<="),
            ">=" => Some(">="),
            _ => None,
        }) {
            tokens.push(Token::Operator(operator));
            index += 2;
            continue;
        }
        let one = match byte {
            b'+' => Some("+"),
            b'-' => Some("-"),
            b'*' => Some("*"),
            b'/' => Some("/"),
            b'%' => Some("%"),
            b'<' => Some("<"),
            b'>' => Some(">"),
            b'!' => Some("!"),
            _ => None,
        };
        if let Some(operator) = one {
            tokens.push(Token::Operator(operator));
            index += 1;
            continue;
        }
        match byte {
            b'(' => tokens.push(Token::Open),
            b')' => tokens.push(Token::Close),
            b'?' => tokens.push(Token::Question),
            b':' => tokens.push(Token::Colon),
            _ => return None,
        }
        index += 1;
    }
    Some(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
}

impl Parser {
    fn peek(&self, expected: &Token) -> bool {
        self.tokens.get(self.position) == Some(expected)
    }

    fn take_operator(&mut self) -> Option<&'static str> {
        match self.tokens.get(self.position) {
            Some(Token::Operator(operator)) => {
                let operator = *operator;
                self.position += 1;
                Some(operator)
            }
            _ => None,
        }
    }

    /// Consume the next token only when it is one of `allowed`; a mismatch
    /// leaves the input untouched for the caller's loop to finish.
    fn take_one_of(&mut self, allowed: &[&str]) -> Option<&'static str> {
        match self.tokens.get(self.position) {
            Some(Token::Operator(operator)) if allowed.contains(operator) => {
                let operator = *operator;
                self.position += 1;
                Some(operator)
            }
            _ => None,
        }
    }

    fn ternary(&mut self) -> Option<Expr> {
        let condition = self.or()?;
        if !self.peek(&Token::Question) {
            return Some(condition);
        }
        self.position += 1;
        let then = self.ternary()?;
        if !self.peek(&Token::Colon) {
            return None;
        }
        self.position += 1;
        let otherwise = self.ternary()?;
        Some(Expr::Ternary(
            Box::new(condition),
            Box::new(then),
            Box::new(otherwise),
        ))
    }

    fn or(&mut self) -> Option<Expr> {
        let mut left = self.and()?;
        while self.take_one_of(&["||"]).is_some() {
            let right = self.and()?;
            left = Expr::Binary(Box::new(left), Operator::Or, Box::new(right));
        }
        Some(left)
    }

    fn and(&mut self) -> Option<Expr> {
        let mut left = self.equality()?;
        while self.take_one_of(&["&&"]).is_some() {
            let right = self.equality()?;
            left = Expr::Binary(Box::new(left), Operator::And, Box::new(right));
        }
        Some(left)
    }

    fn equality(&mut self) -> Option<Expr> {
        let mut left = self.relational()?;
        while let Some(operator) = self.take_one_of(&["==", "!="]) {
            let operator = if operator == "==" {
                Operator::Eq
            } else {
                Operator::Ne
            };
            let right = self.relational()?;
            left = Expr::Binary(Box::new(left), operator, Box::new(right));
        }
        Some(left)
    }

    fn relational(&mut self) -> Option<Expr> {
        let mut left = self.additive()?;
        while let Some(operator) = self.take_one_of(&["<", "<=", ">", ">="]) {
            let operator = match operator {
                "<" => Operator::Lt,
                "<=" => Operator::Le,
                ">" => Operator::Gt,
                _ => Operator::Ge,
            };
            let right = self.additive()?;
            left = Expr::Binary(Box::new(left), operator, Box::new(right));
        }
        Some(left)
    }

    fn additive(&mut self) -> Option<Expr> {
        let mut left = self.multiplicative()?;
        while let Some(operator) = self.take_one_of(&["+", "-"]) {
            let operator = if operator == "+" {
                Operator::Add
            } else {
                Operator::Sub
            };
            let right = self.multiplicative()?;
            left = Expr::Binary(Box::new(left), operator, Box::new(right));
        }
        Some(left)
    }

    fn multiplicative(&mut self) -> Option<Expr> {
        let mut left = self.unary()?;
        while let Some(operator) = self.take_one_of(&["*", "/", "%"]) {
            let operator = match operator {
                "*" => Operator::Mul,
                "/" => Operator::Div,
                _ => Operator::Rem,
            };
            let right = self.unary()?;
            left = Expr::Binary(Box::new(left), operator, Box::new(right));
        }
        Some(left)
    }

    fn unary(&mut self) -> Option<Expr> {
        match self.take_operator() {
            Some("!") => Some(Expr::Not(Box::new(self.unary()?))),
            Some("-") => Some(Expr::Neg(Box::new(self.unary()?))),
            Some(_) => {
                self.position -= 1;
                self.primary()
            }
            None => self.primary(),
        }
    }

    fn primary(&mut self) -> Option<Expr> {
        match self.tokens.get(self.position)?.clone() {
            Token::Number(value) => {
                self.position += 1;
                Some(Expr::Number(value))
            }
            Token::N => {
                self.position += 1;
                Some(Expr::N)
            }
            Token::Open => {
                self.position += 1;
                let expression = self.ternary()?;
                if !self.peek(&Token::Close) {
                    return None;
                }
                self.position += 1;
                Some(expression)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a GNU MO file with little-endian tables. The test does not depend
    /// on msgfmt being installed, which matters on Windows CI.
    fn mo(entries: &[(&str, &str)], header: &str) -> Vec<u8> {
        let mut strings: Vec<(String, String)> = vec![("".to_string(), header.to_string())];
        strings.extend(
            entries
                .iter()
                .map(|(id, value)| ((*id).to_string(), (*value).to_string())),
        );
        let count = strings.len() as u32;
        let originals_offset = 28u32;
        let translations_offset = originals_offset + count * 8;
        let strings_offset = translations_offset + count * 8;
        let mut payload = Vec::new();
        let mut originals = Vec::new();
        let mut translations = Vec::new();
        for (id, value) in &strings {
            let id_offset = strings_offset + payload.len() as u32;
            payload.extend_from_slice(id.as_bytes());
            originals.push((id.len() as u32, id_offset));
            let value_offset = strings_offset + payload.len() as u32;
            payload.extend_from_slice(value.as_bytes());
            translations.push((value.len() as u32, value_offset));
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0x9504_12deu32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&originals_offset.to_le_bytes());
        bytes.extend_from_slice(&translations_offset.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        for (length, offset) in originals {
            bytes.extend_from_slice(&length.to_le_bytes());
            bytes.extend_from_slice(&offset.to_le_bytes());
        }
        for (length, offset) in translations {
            bytes.extend_from_slice(&length.to_le_bytes());
            bytes.extend_from_slice(&offset.to_le_bytes());
        }
        bytes.extend_from_slice(&payload);
        bytes
    }

    #[test]
    fn init_does_not_deadlock_on_the_translator_lock() {
        // Regression: `init` used to hold the write guard while calling a
        // helper that read the same lock, so the process stopped at startup
        // before printing anything.
        init(std::env::temp_dir());
    }

    #[test]
    fn translations_round_trip_through_a_mo_catalog() {
        let catalog = Catalog::parse(&mo(
            &[("Active", "Активно"), ("Add", "Добавить")],
            "Language: ru\nPlural-Forms: nplurals=3; plural=(n%10==1 ? 0 : 1);\n",
        ))
        .unwrap();
        assert_eq!(catalog.messages["Active"], vec!["Активно".to_string()]);
        assert_eq!(catalog.messages["Add"], vec!["Добавить".to_string()]);
    }

    #[test]
    fn a_missing_message_falls_back_to_its_source_string() {
        let catalog = Catalog::parse(&mo(&[], "Language: ru\n")).unwrap();
        assert!(!catalog.messages.contains_key("Active"));
        assert_eq!(catalog.plural.index(2), 1);
    }

    #[test]
    fn russian_plural_rules_choose_the_documented_forms() {
        // The exact expression shipped in po/ru.po.
        let header = "Plural-Forms: nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : \
                      n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2);\n";
        let rule = PluralRule::parse_header(header);
        for (n, expected) in [
            (1, 0),
            (2, 1),
            (4, 1),
            (5, 2),
            (11, 2),
            (21, 0),
            (22, 1),
            (25, 2),
        ] {
            assert_eq!(rule.index(n), expected, "n = {n}");
        }
    }

    #[test]
    fn a_plural_msgid_is_keyed_by_its_singular_form() {
        // MO stores a plural entry as `singular\0plural`; `tr_n` asks for the
        // singular. Regression: the raw key never matched, so counted nouns
        // stayed English in Russian and Japanese.
        let catalog = Catalog::parse(&mo(
            &[("display\0displays", "дисплей\0дисплея\0дисплеев")],
            "Plural-Forms: nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : \
             n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2);\n",
        ))
        .unwrap();
        assert_eq!(
            catalog.messages["display"],
            vec![
                "дисплей".to_string(),
                "дисплея".to_string(),
                "дисплеев".to_string()
            ]
        );
        assert!(!catalog.messages.contains_key("display\0displays"));
    }

    #[test]
    fn a_single_form_catalog_always_uses_form_zero() {
        let rule = PluralRule::parse_header("Plural-Forms: nplurals=1; plural=0;\n");
        for n in [0, 1, 2, 11, 100] {
            assert_eq!(rule.index(n), 0);
        }
    }

    #[test]
    fn a_malformed_plural_expression_does_not_panic() {
        let rule = PluralRule::parse_header("Plural-Forms: nplurals=2; plural=(n ?;\n");
        assert_eq!(rule.index(1), 0);
        assert_eq!(rule.index(7), 1);
    }

    #[test]
    fn divisor_and_remainder_guards_keep_evaluation_total() {
        let expression = Expr::parse("10 / (n - n) + 7 % 0").unwrap();
        assert_eq!(expression.eval(4), 0);
    }

    #[test]
    fn a_truncated_file_is_rejected_instead_of_panicking() {
        assert!(Catalog::parse(b"").is_none());
        assert!(Catalog::parse(&[0xde, 0x12, 0x04, 0x95]).is_none());
        let mut bytes = mo(&[("A", "B")], "");
        bytes.truncate(20);
        assert!(Catalog::parse(&bytes).is_none());
    }

    #[test]
    fn the_language_parser_strips_encoding_and_modifier() {
        assert_eq!(
            normalise_language("ru_RU.UTF-8@euro"),
            Some("ru_RU".to_string())
        );
        assert_eq!(normalise_language("ja-JP"), Some("ja_JP".to_string()));
        assert_eq!(normalise_language("C"), None);
        assert_eq!(normalise_language("POSIX"), None);
    }
}
