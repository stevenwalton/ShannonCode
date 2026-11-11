//! Render-agnostic text styling — the seam between the output cache and the
//! renderer.
//!
//! The output pane caches **styled** rows (markdown emphasis + syntax-
//! highlighted code, computed once at commit). To keep `output.rs` free of any
//! `ratatui` dependency (the design's model/view split), the cache stores
//! [`CellStyle`] — a tiny render-agnostic style (a highlight role + a few
//! attributes) — and the active theme (`theme::Theme`) maps it to a concrete
//! `ratatui::Style` at the edge. `CellStyle::default()` is "plain" (terminal
//! default), so an unstyled run costs nothing and renders exactly as before.
//! A role, not a colour, is cached: a new theme recolours what is already in
//! the transcript without highlighting it again.

/// Declares [`Ink`] once — each role with its dotted name — so [`Ink::ALL`]
/// lists every variant by construction, never by a second hand-kept list.
macro_rules! inks {
    ($($(#[$doc:meta])* $variant:ident => $name:literal,)*) => {
        /// **A highlight role** — what a piece of text IS (a keyword, a
        /// string, a command's flag, a line a write adds), never what colour
        /// it is: the theme decides that (`theme::Theme`). One vocabulary for
        /// every highlighter — the gate's shell lexer, syntect, markdown,
        /// and (next) a language server's semantic tokens and diagnostics
        /// ([`Ink::from_lsp`]) — so the same thing reads the same wherever
        /// it appears.
        ///
        /// Named as tree-sitter's highlight captures are (`@function.call`,
        /// `@variable.parameter`, `@markup.raw`): the vocabulary neovim,
        /// helix and zed themes already share, and the one a language
        /// server's token types map onto. A theme may name a role or any
        /// prefix of it — `function` paints `function.call` unless that is
        /// named itself.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Ink {
            $($(#[$doc])* $variant,)*
        }

        impl Ink {
            /// Every role, in declaration order — the index of each is
            /// `ink as usize`.
            pub const ALL: &'static [Ink] = &[$(Ink::$variant,)*];

            /// The role's dotted name, as a theme writes it.
            pub const fn name(self) -> &'static str {
                match self {
                    $(Ink::$variant => $name,)*
                }
            }
        }
    };
}

inks! {
    /// What ran, or was asked to: a tool record's header and command.
    Tool => "ui.tool",
    /// Quiet within a record: the line numbers before a read's text.
    Muted => "ui.muted",
    /// A line a write adds.
    Added => "diff.plus",
    /// A line a write removes.
    Removed => "diff.minus",
    /// A comment.
    Comment => "comment",
    /// A reserved word: `for`, `do`, `if`, `fn`, `let`.
    Keyword => "keyword",
    /// A string — quoted, in shell.
    String => "string",
    /// What is special inside one: an escape, a regex.
    StringSpecial => "string.special",
    /// A number.
    Number => "number",
    /// A constant.
    Constant => "constant",
    /// A constant the language gives: `true`, `nil`, `None`.
    ConstantBuiltin => "constant.builtin",
    /// A function's name where it is defined or named.
    Function => "function",
    /// A function called — in shell, the command's name.
    FunctionCall => "function.call",
    /// A function the language gives.
    FunctionBuiltin => "function.builtin",
    /// A macro.
    FunctionMacro => "function.macro",
    /// A type.
    Type => "type",
    /// A type the language gives.
    TypeBuiltin => "type.builtin",
    /// A variable.
    Variable => "variable",
    /// A variable the language gives: `self`, `this`.
    VariableBuiltin => "variable.builtin",
    /// A parameter — in shell, a command's flag (`-n`, `--glob=…`).
    Parameter => "variable.parameter",
    /// What the shell expands: `$NAME`, `${…}`, `$(…)`, `NAME=value`.
    VariableSpecial => "variable.special",
    /// A field or property.
    Member => "variable.member",
    /// A module or namespace.
    Module => "module",
    /// An attribute or decorator.
    Attribute => "attribute",
    /// A markup tag.
    Tag => "tag",
    /// A label.
    Label => "label",
    /// `|`, `&&`, `;`, a redirect; `+`, `==`.
    Operator => "operator",
    /// Brackets and delimiters.
    Punctuation => "punctuation",
    /// A markdown heading.
    Heading => "markup.heading",
    /// Code in prose: an inline span.
    Raw => "markup.raw",
    /// A code block's fences.
    RawFence => "markup.raw.fence",
    /// A diagnostic, by severity.
    Error => "diagnostic.error",
    /// —
    Warning => "diagnostic.warning",
    /// —
    Info => "diagnostic.info",
    /// —
    Hint => "diagnostic.hint",

    // **The UI's own roles** (Steven, 10-14: "If there's a color defined in
    // our program, we should let someone be able to specify how *they* want
    // that drawn"). What `palette.rs` named, each a role a theme paints —
    // appended, so the highlighters' indices above do not move.

    /// Every pane's ground: its background, and the colour of text with no
    /// role. Unset in the shipped theme — the terminal's own.
    Background => "ui.background",
    /// Ordinary text the user is meant to read: the status row's, a page's.
    Text => "ui.text",
    /// Quieter: prose around a marked run, an empty page's message.
    TextQuiet => "ui.text.quiet",
    /// Quietest: the app talking about a message rather than saying it.
    TextFaint => "ui.text.faint",
    /// Rows past a page's end — vim's `~`, in its `NonText` colour.
    NonText => "ui.nontext",
    /// The status row's text.
    Statusline => "ui.statusline",
    /// An unfocused pane's border and title.
    Border => "ui.border",
    /// The focused pane's border and title.
    BorderFocused => "ui.border.focused",
    /// A pane the prompt is aimed at, not focused.
    BorderAimed => "ui.border.aimed",
    /// The rules above and below your message in a transcript. Not in the
    /// shipped table: unset, a theme frames it in its own `ui.border`.
    BorderUser => "ui.border.user",
    /// A completion menu's chosen candidate.
    MenuSelected => "ui.menu.selected",
    /// The line a screen's cursor is on: `:models`, `:effort`, setup.
    Chosen => "ui.chosen",
    /// A screen's bar — its keys, as vim's command-line window has one.
    Bar => "ui.bar",
    /// The bar while a decision waits behind the screen.
    BarWaiting => "ui.bar.waiting",
    /// Benign: an allowed command, a passing check.
    Safe => "ui.safe",
    /// Read before continuing: an `ask`, a report of something done.
    Caution => "ui.caution",
    /// Stop: a denied command, a refusal, an unrecoverable action.
    Danger => "ui.danger",
    /// An unbounded action's warning sign: the one filled tag.
    DangerSign => "ui.danger.sign",
    /// The decision pane's block on the words an `ask` stands on.
    CautionFill => "ui.caution.fill",
    /// The decision pane's block on the words a `deny` stands on.
    DangerFill => "ui.danger.fill",
    /// A question whose safe answer may be `y` — not an alarm.
    Question => "ui.question",
    /// What an action lands on: the id, the count, the secret.
    Operand => "ui.operand",
    /// A vaulted secret's handle where its value would have been.
    Secret => "ui.secret",
    /// `:graph`'s connector glyphs.
    GraphTree => "ui.graph.tree",
    /// A session with a turn in flight.
    GraphRunning => "ui.graph.running",
    /// A session parked on a decision.
    GraphHeld => "ui.graph.held",
    /// A row whose turns are another session's.
    GraphShared => "ui.graph.shared",
    /// A marked row (`mf`): its background.
    GraphMark => "ui.graph.mark",
    /// A Claude Code conversation a Shannon session continues: its mark.
    GraphContinued => "ui.graph.continued",
    /// `:graph`'s lanes, cycled by lane.
    Lane1 => "ui.graph.lane.1",
    /// —
    Lane2 => "ui.graph.lane.2",
    /// —
    Lane3 => "ui.graph.lane.3",
    /// —
    Lane4 => "ui.graph.lane.4",
    /// —
    Lane5 => "ui.graph.lane.5",
    /// —
    Lane6 => "ui.graph.lane.6",
    /// The analyser's data carriers, cycled by carrier.
    Carrier1 => "ui.analyze.carrier.1",
    /// —
    Carrier2 => "ui.analyze.carrier.2",
    /// —
    Carrier3 => "ui.analyze.carrier.3",
    /// —
    Carrier4 => "ui.analyze.carrier.4",
    /// A section heading in the manual.
    HelpHeading => "ui.help.heading",
    /// A `*tag*` in the manual.
    HelpTag => "ui.help.tag",
    /// A `|reference|` in the manual.
    HelpRef => "ui.help.ref",
    /// What you type: an inline `code` span, a listing's reference.
    HelpCode => "ui.help.code",
    /// A section heading on a listing page (`:rules`).
    ListingHeading => "ui.listing.heading",
    /// A held secret's value in text the MODEL wrote, not put back from its
    /// handle — the model had the value. Drawn underlined, whatever the
    /// theme (ratatui has no undercurl); unset, it is `ui.secret`'s colour
    /// (Steven, 10-19: "an underline along with a color"; review stop 6, F 5).
    SecretLeaked => "ui.secret.leaked",
    /// The tab line: the tabs but yours, and its ground (vim's `TabLine`
    /// and `TabLineFill`).
    TabLine => "ui.tabline",
    /// The tab you are in, on the tab line (vim's `TabLineSel`).
    TabLineSelected => "ui.tabline.selected",
    /// What the model thought, in a transcript (N1b): quieter than the
    /// reply it led to, and apart from a tool's output, which is dim too —
    /// in the record's colour (`ToolOutput`), and upright.
    /// Not in the shipped table: unset, it is the theme's `ui.text.quiet`,
    /// so a light theme that repaints quiet text repaints it too (in
    /// `terminal`'s grey it was Solarized light's ground, 1.14:1). Its
    /// italic and dim are the view's (`view::THINKING`).
    Thinking => "ui.text.quiet.thinking",
    /// What came of a tool call — a command's output, a call's result —
    /// told from the reply and from thinking at a glance (Steven, 11-01:
    /// "it should be different from normal text and from thinking"; in
    /// `ui.muted`'s grey it read as the reply). Not in the shipped table:
    /// unset, it is the theme's `ui.tool`, so a record reads as one thing in
    /// every theme; its dim is the record's (`tool_record::OUTPUT`).
    ToolOutput => "ui.tool.output",
    /// The terminal's cursor, the one cursor in every pane: its `bg` (or a
    /// bare colour) is the block's — vim's `Cursor`, whose guibg the TUI
    /// sends. Unset where a theme paints a ground, it is the ground's text;
    /// with no ground, the terminal's own (`Theme::cursor`).
    Cursor => "ui.cursor",
}

impl Ink {
    /// `:graph`'s lane roles, cycled by lane index.
    pub const LANES: [Ink; 6] = [
        Ink::Lane1,
        Ink::Lane2,
        Ink::Lane3,
        Ink::Lane4,
        Ink::Lane5,
        Ink::Lane6,
    ];

    /// The analyser's carrier roles, cycled per carrier.
    pub const CARRIERS: [Ink; 4] = [Ink::Carrier1, Ink::Carrier2, Ink::Carrier3, Ink::Carrier4];

    /// **The role a language server's semantic token is** — its standard
    /// token type (LSP 3.17 `SemanticTokenTypes`), with `defaultLibrary`
    /// among its modifiers marking what the language gives. The seam a
    /// language server lands on: its tokens go through this vocabulary, so a
    /// theme paints them as it paints everything else. `None` for a type
    /// outside the standard set (a server's own extension).
    ///
    /// **Not yet wired: there is no LSP client.** A seam kept on purpose —
    /// it is what makes a server's arrival add a producer rather than a
    /// parallel vocabulary (`docs/DesignDocs/themes.md`) — and held to its
    /// mapping by a test until a client consumes it.
    pub fn from_lsp(token_type: &str, modifiers: &[&str]) -> Option<Ink> {
        let builtin = modifiers.contains(&"defaultLibrary");
        Some(match token_type {
            "namespace" => Ink::Module,
            "type" | "class" | "enum" | "interface" | "struct" | "typeParameter" => {
                if builtin {
                    Ink::TypeBuiltin
                } else {
                    Ink::Type
                }
            }
            "parameter" => Ink::Parameter,
            "variable" if builtin => Ink::VariableBuiltin,
            "variable" => Ink::Variable,
            "property" | "event" => Ink::Member,
            "enumMember" => Ink::Constant,
            "function" | "method" if builtin => Ink::FunctionBuiltin,
            "function" | "method" => Ink::Function,
            "macro" => Ink::FunctionMacro,
            "keyword" | "modifier" => Ink::Keyword,
            "comment" => Ink::Comment,
            "string" => Ink::String,
            "number" => Ink::Number,
            "regexp" => Ink::StringSpecial,
            "operator" => Ink::Operator,
            "decorator" => Ink::Attribute,
            _ => return None,
        })
    }
}

/// A render-agnostic character style: a highlight role, plus emphasis flags
/// the text itself carries (a markdown `**bold**`). `default()` is plain (no
/// role, no attributes) and is the overwhelmingly common run, so the cache
/// and the styled wrap stay cheap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellStyle {
    pub ink: Option<Ink>,
    pub bold: bool,
    pub italic: bool,
    pub dim: bool,
}

impl CellStyle {
    /// A plain style (terminal default) — the unstyled run.
    pub const PLAIN: CellStyle = CellStyle {
        ink: None,
        bold: false,
        italic: false,
        dim: false,
    };

    /// A style with just a highlight role.
    pub const fn ink(ink: Ink) -> Self {
        CellStyle {
            ink: Some(ink),
            ..CellStyle::PLAIN
        }
    }

    /// This style with `bold` set — builder for the few emphasis cases.
    pub const fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    /// This style with `italic` set.
    pub const fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    /// This style with `dim` set.
    pub const fn dim(mut self) -> Self {
        self.dim = true;
        self
    }

    /// Whether this is the plain (default) style — lets the renderer skip
    /// building a `Style` for the common unstyled run.
    pub fn is_plain(&self) -> bool {
        *self == CellStyle::PLAIN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **`Ink::ALL` is indexed by the role** — `highlight` carries a role
    /// through syntect as `ink as u8` and reads it back with `Ink::ALL.get`,
    /// and a theme's tables are indexed the same way — and each role's
    /// name is its own.
    #[test]
    fn every_role_is_at_its_index_with_a_name_of_its_own() {
        assert!(Ink::ALL.len() <= usize::from(u8::MAX));
        for (i, &ink) in Ink::ALL.iter().enumerate() {
            assert_eq!(ink as usize, i, "{ink:?}");
            let same = Ink::ALL.iter().filter(|o| o.name() == ink.name()).count();
            assert_eq!(same, 1, "{}", ink.name());
        }
    }

    /// **Every standard LSP token type is a role** (3.17's
    /// `SemanticTokenTypes`), the language's own marked by
    /// `defaultLibrary`; a server's extension is none.
    #[test]
    fn every_standard_lsp_token_type_is_a_role() {
        let standard = [
            "namespace",
            "type",
            "class",
            "enum",
            "interface",
            "struct",
            "typeParameter",
            "parameter",
            "variable",
            "property",
            "enumMember",
            "event",
            "function",
            "method",
            "macro",
            "keyword",
            "modifier",
            "comment",
            "string",
            "number",
            "regexp",
            "operator",
            "decorator",
        ];
        for kind in standard {
            assert!(Ink::from_lsp(kind, &[]).is_some(), "{kind}");
        }
        // **The mapping, every type** — the doc says a test holds it, and
        // `is_some` for each held only that it maps to SOMETHING: `number` as
        // a string passed (review stop 12, T7). No client reads it yet, so
        // this table is all that does.
        let roles = [
            ("namespace", Ink::Module),
            ("type", Ink::Type),
            ("class", Ink::Type),
            ("enum", Ink::Type),
            ("interface", Ink::Type),
            ("struct", Ink::Type),
            ("typeParameter", Ink::Type),
            ("parameter", Ink::Parameter),
            ("variable", Ink::Variable),
            ("property", Ink::Member),
            ("enumMember", Ink::Constant),
            ("event", Ink::Member),
            ("function", Ink::Function),
            ("method", Ink::Function),
            ("macro", Ink::FunctionMacro),
            ("keyword", Ink::Keyword),
            ("modifier", Ink::Keyword),
            ("comment", Ink::Comment),
            ("string", Ink::String),
            ("number", Ink::Number),
            ("regexp", Ink::StringSpecial),
            ("operator", Ink::Operator),
            ("decorator", Ink::Attribute),
        ];
        assert_eq!(roles.len(), standard.len(), "premise: one row a type");
        for (kind, ink) in roles {
            assert_eq!(Ink::from_lsp(kind, &[]), Some(ink), "{kind}");
        }
        for (kind, ink) in [
            ("type", Ink::TypeBuiltin),
            ("variable", Ink::VariableBuiltin),
            ("method", Ink::FunctionBuiltin),
        ] {
            assert_eq!(
                Ink::from_lsp(kind, &["defaultLibrary"]),
                Some(ink),
                "{kind}, builtin"
            );
        }
        assert_eq!(Ink::from_lsp("function", &[]), Some(Ink::Function));
        assert_eq!(
            Ink::from_lsp("function", &["defaultLibrary"]),
            Some(Ink::FunctionBuiltin)
        );
        assert_eq!(
            Ink::from_lsp("struct", &["defaultLibrary"]),
            Some(Ink::TypeBuiltin)
        );
        assert_eq!(
            Ink::from_lsp("parameter", &["readonly"]),
            Some(Ink::Parameter)
        );
        assert_eq!(Ink::from_lsp("lifetime", &[]), None);
    }
}
