//! Renderer for Neovim: colours in, the text of `colors/velatura.lua` out,
//! and of a table of the same colours for people who keep a colourscheme of
//! their own.
//!
//! The colourscheme stands by itself: it needs no plugin, and it is a list
//! of calls with fixed names, so nothing from a theme file other than its
//! colours can end up in it.

use std::fmt::Write;

use crate::model::{ANSI_NAMES, DIAGNOSTIC_ROLES, DIFF_ROLES, SYNTAX_ROLES};
use crate::{Appearance, Color, Resolved};

/// Bumped whenever the set of groups or their mapping changes.
pub const FORMAT: u32 = 1;

/// The name `:colorscheme` takes.
pub const NAME: &str = "velatura";

/// One highlight group. `sp` is the colour of an undercurl.
#[derive(Clone, Copy, Default)]
struct Hl {
    fg: Option<Color>,
    bg: Option<Color>,
    sp: Option<Color>,
    bold: bool,
    italic: bool,
    underline: bool,
}

fn fg(c: Color) -> Hl {
    Hl {
        fg: Some(c),
        ..Hl::default()
    }
}

fn bg(c: Color) -> Hl {
    Hl {
        bg: Some(c),
        ..Hl::default()
    }
}

fn on(fg: Color, bg: Color) -> Hl {
    Hl {
        fg: Some(fg),
        bg: Some(bg),
        ..Hl::default()
    }
}

fn curl(c: Color) -> Hl {
    Hl {
        sp: Some(c),
        ..Hl::default()
    }
}

impl Hl {
    fn bold(self) -> Self {
        Self { bold: true, ..self }
    }
    fn italic(self) -> Self {
        Self {
            italic: true,
            ..self
        }
    }
    fn underline(self) -> Self {
        Self {
            underline: true,
            ..self
        }
    }

    fn lua(&self) -> String {
        let mut parts = Vec::new();
        for (key, color) in [("fg", self.fg), ("bg", self.bg), ("sp", self.sp)] {
            if let Some(color) = color {
                parts.push(format!("{key} = \"{}\"", color.hex()));
            }
        }
        for (key, set) in [
            ("bold", self.bold),
            ("italic", self.italic),
            ("underline", self.underline),
            ("undercurl", self.sp.is_some()),
        ] {
            if set {
                parts.push(format!("{key} = true"));
            }
        }
        format!("{{ {} }}", parts.join(", "))
    }
}

/// Groups that take the look of another: `(group, the one it follows)`.
const LINKS: [(&str, &str); 37] = [
    ("NormalNC", "Normal"),
    ("CursorColumn", "CursorLine"),
    ("VertSplit", "WinSeparator"),
    ("CurSearch", "IncSearch"),
    ("Character", "String"),
    ("Float", "Number"),
    ("Conditional", "Statement"),
    ("Repeat", "Statement"),
    ("Label", "Statement"),
    ("Keyword", "Statement"),
    ("Exception", "Statement"),
    ("Include", "PreProc"),
    ("Define", "PreProc"),
    ("Macro", "PreProc"),
    ("StorageClass", "Type"),
    ("Structure", "Type"),
    ("Typedef", "Type"),
    ("@constant", "Constant"),
    ("@keyword", "Keyword"),
    ("@string", "String"),
    ("@number", "Number"),
    ("@function", "Function"),
    ("@type", "Type"),
    ("@operator", "Operator"),
    ("@comment", "Comment"),
    ("@lsp.type.parameter", "@variable.parameter"),
    ("@lsp.type.property", "@property"),
    ("@lsp.type.namespace", "@module"),
    ("TelescopeBorder", "FloatBorder"),
    ("TelescopeNormal", "NormalFloat"),
    ("TelescopeSelection", "PmenuSel"),
    ("NeoTreeNormal", "Normal"),
    ("NvimTreeNormal", "Normal"),
    ("LazyNormal", "NormalFloat"),
    ("MasonNormal", "NormalFloat"),
    ("WhichKeyFloat", "NormalFloat"),
    ("BlinkCmpMenu", "Pmenu"),
];

/// Every group that is given colours, in output order.
fn groups(theme: &Resolved<'_>) -> Vec<(&'static str, Hl)> {
    let v = theme.colors;
    let ui = &v.ui;
    let s = |role| v.syntax_color(role);
    let d = |role| v.diagnostic_color(role);
    vec![
        // The editor itself.
        ("Normal", on(ui.fg, ui.bg)),
        ("NormalFloat", on(ui.fg, ui.surface)),
        ("FloatBorder", on(ui.border, ui.surface)),
        ("FloatTitle", on(ui.accent, ui.surface).bold()),
        ("ColorColumn", bg(ui.surface)),
        ("Conceal", fg(ui.fg_subtle)),
        ("Cursor", on(ui.cursor_fg, ui.cursor)),
        ("CursorLine", bg(ui.surface)),
        ("CursorLineNr", fg(ui.accent).bold()),
        ("LineNr", fg(ui.fg_subtle)),
        ("SignColumn", fg(ui.fg_subtle)),
        ("FoldColumn", fg(ui.fg_subtle)),
        ("Folded", on(ui.fg_muted, ui.surface)),
        ("WinSeparator", fg(ui.border)),
        ("StatusLine", on(ui.fg, ui.bg_dim)),
        ("StatusLineNC", on(ui.fg_subtle, ui.bg_dim)),
        ("TabLine", on(ui.fg_subtle, ui.bg_dim)),
        ("TabLineFill", bg(ui.bg_dim)),
        ("TabLineSel", on(ui.fg, ui.bg).bold()),
        ("WinBar", fg(ui.fg_muted).bold()),
        ("WinBarNC", fg(ui.fg_subtle)),
        ("Pmenu", on(ui.fg, ui.surface)),
        ("PmenuSel", on(ui.selection_fg, ui.selection_bg)),
        ("PmenuSbar", bg(ui.surface)),
        ("PmenuThumb", bg(ui.border)),
        ("Visual", bg(ui.selection_bg)),
        ("Search", on(ui.bg, ui.warning)),
        ("IncSearch", on(ui.accent_fg, ui.accent)),
        ("Substitute", on(ui.bg, ui.error)),
        ("MatchParen", fg(ui.accent).bold()),
        ("NonText", fg(ui.border)),
        ("Whitespace", fg(ui.border)),
        ("SpecialKey", fg(ui.fg_subtle)),
        ("EndOfBuffer", fg(ui.bg)),
        ("Directory", fg(s("function"))),
        ("Title", fg(ui.accent).bold()),
        ("ErrorMsg", fg(ui.error)),
        ("WarningMsg", fg(ui.warning)),
        ("MoreMsg", fg(ui.success)),
        ("ModeMsg", fg(ui.fg_muted).bold()),
        ("Question", fg(ui.success)),
        ("QuickFixLine", bg(ui.surface)),
        ("SpellBad", curl(ui.error)),
        ("SpellCap", curl(ui.warning)),
        ("SpellRare", curl(d("info"))),
        ("SpellLocal", curl(d("hint"))),
        // Changes.
        ("DiffAdd", bg(v.diff_color("add"))),
        ("DiffChange", bg(v.diff_color("change"))),
        ("DiffDelete", on(ui.fg_subtle, v.diff_color("delete"))),
        ("DiffText", bg(v.diff_color("text"))),
        ("Added", fg(ui.success)),
        ("Changed", fg(ui.warning)),
        ("Removed", fg(ui.error)),
        // Code.
        ("Comment", fg(s("comment")).italic()),
        ("Constant", fg(s("constant"))),
        ("String", fg(s("string"))),
        ("Number", fg(s("number"))),
        ("Boolean", fg(s("constant")).bold()),
        ("Identifier", fg(s("variable"))),
        ("Function", fg(s("function"))),
        ("Statement", fg(s("keyword"))),
        ("Operator", fg(s("operator"))),
        ("PreProc", fg(s("attribute"))),
        ("Type", fg(s("type"))),
        ("Special", fg(s("builtin"))),
        ("SpecialChar", fg(s("escape"))),
        ("Tag", fg(s("tag"))),
        ("Delimiter", fg(s("punctuation"))),
        ("SpecialComment", fg(ui.fg_muted).italic()),
        ("Debug", fg(ui.error)),
        ("Underlined", fg(ui.link).underline()),
        ("Error", fg(ui.error)),
        ("Todo", on(ui.bg, d("info")).bold()),
        ("@variable", fg(s("variable"))),
        ("@variable.builtin", fg(s("builtin"))),
        ("@variable.parameter", fg(s("parameter"))),
        ("@variable.member", fg(s("property"))),
        ("@property", fg(s("property"))),
        ("@constant.builtin", fg(s("constant")).bold()),
        ("@module", fg(s("type"))),
        ("@string.escape", fg(s("escape"))),
        ("@string.regexp", fg(s("escape"))),
        ("@function.builtin", fg(s("builtin"))),
        ("@constructor", fg(s("type"))),
        ("@type.builtin", fg(s("type"))),
        ("@attribute", fg(s("attribute"))),
        ("@punctuation.delimiter", fg(s("punctuation"))),
        ("@punctuation.bracket", fg(s("punctuation"))),
        ("@punctuation.special", fg(s("escape"))),
        ("@tag", fg(s("tag"))),
        ("@tag.attribute", fg(s("attribute"))),
        ("@tag.delimiter", fg(s("punctuation"))),
        ("@markup.heading", fg(ui.accent).bold()),
        ("@markup.strong", Hl::default().bold()),
        ("@markup.italic", Hl::default().italic()),
        ("@markup.raw", fg(s("string"))),
        ("@markup.link", fg(ui.link)),
        ("@markup.link.url", fg(ui.link).underline()),
        // Problems.
        ("DiagnosticError", fg(d("error"))),
        ("DiagnosticWarn", fg(d("warning"))),
        ("DiagnosticInfo", fg(d("info"))),
        ("DiagnosticHint", fg(d("hint"))),
        ("DiagnosticOk", fg(d("ok"))),
        ("DiagnosticUnderlineError", curl(d("error"))),
        ("DiagnosticUnderlineWarn", curl(d("warning"))),
        ("DiagnosticUnderlineInfo", curl(d("info"))),
        ("DiagnosticUnderlineHint", curl(d("hint"))),
        ("DiagnosticUnderlineOk", curl(d("ok"))),
        // Widely used plugins.
        ("GitSignsAdd", fg(ui.success)),
        ("GitSignsChange", fg(ui.warning)),
        ("GitSignsDelete", fg(ui.error)),
        ("TelescopeMatching", fg(ui.accent).bold()),
        ("TelescopePromptPrefix", fg(ui.accent)),
        ("WhichKey", fg(ui.accent)),
        ("WhichKeyDesc", fg(ui.fg)),
        ("WhichKeyGroup", fg(s("keyword"))),
        ("IblIndent", fg(ui.border)),
        ("IblScope", fg(ui.fg_subtle)),
        ("CmpItemAbbrMatch", fg(ui.accent).bold()),
        ("CmpItemAbbrMatchFuzzy", fg(ui.accent)),
        ("CmpItemKind", fg(s("type"))),
        ("BlinkCmpLabelMatch", fg(ui.accent).bold()),
        ("NeoTreeDirectoryIcon", fg(s("function"))),
        ("NvimTreeFolderIcon", fg(s("function"))),
        ("LazyH1", on(ui.accent_fg, ui.accent).bold()),
        (
            "MiniStatuslineModeNormal",
            on(ui.accent_fg, ui.accent).bold(),
        ),
    ]
}

fn header(out: &mut String, theme: &Resolved<'_>) {
    out.push_str("-- Generated by Velatura. Edits to this file are replaced on the next apply.\n");
    let _ = writeln!(
        out,
        "-- theme {} fingerprint {} format {FORMAT}",
        theme.label(),
        theme.fingerprint()
    );
}

fn background(theme: &Resolved<'_>) -> &'static str {
    match theme.colors.appearance {
        Appearance::Dark => "dark",
        Appearance::Light => "light",
    }
}

/// The colourscheme: `:colorscheme velatura`.
pub fn render(theme: &Resolved<'_>) -> String {
    let mut out = String::new();
    header(&mut out, theme);
    out.push_str("vim.cmd(\"highlight clear\")\n");
    // Without a name, setting the background does not load a scheme again.
    out.push_str("vim.g.colors_name = nil\n");
    let _ = writeln!(out, "vim.o.background = \"{}\"", background(theme));
    let _ = writeln!(out, "vim.g.colors_name = \"{NAME}\"");
    out.push_str("local hl = vim.api.nvim_set_hl\n");
    for (name, spec) in groups(theme) {
        let _ = writeln!(out, "hl(0, \"{name}\", {})", spec.lua());
    }
    for (name, to) in LINKS {
        let _ = writeln!(out, "hl(0, \"{name}\", {{ link = \"{to}\" }})");
    }
    for (i, color) in theme.colors.ansi.iter().enumerate() {
        let _ = writeln!(out, "vim.g.terminal_color_{i} = \"{}\"", color.hex());
    }
    out.push_str(RELOAD);
    out
}

/// Velatura writes this file again when another theme is applied; a
/// Neovim already open loads it again the next time its window is
/// focused. Nested, so the user's own `ColorScheme` autocommands run too.
/// Written with the same few characters as the rest of the file.
const RELOAD: &str = r#"local uv = vim.uv or vim.loop
local file = string.sub(debug.getinfo(1, "S").source, 2)
local function stamp()
  local s = uv.fs_stat(file)
  return s and (s.mtime.sec .. "." .. s.mtime.nsec)
end
local loaded = stamp()
vim.api.nvim_create_autocmd("FocusGained", {
  group = vim.api.nvim_create_augroup("velatura", { clear = true }),
  nested = true,
  callback = function()
    if vim.g.colors_name == "velatura" and not (stamp() == loaded) then
      vim.cmd.colorscheme("velatura")
    end
  end,
})
"#;

/// The same colours as a table, for `require("velatura.palette")`.
pub fn render_palette(theme: &Resolved<'_>) -> String {
    let v = theme.colors;
    let mut out = String::new();
    header(&mut out, theme);
    out.push_str("return {\n");
    let _ = writeln!(out, "  appearance = \"{}\",", background(theme));
    for (name, color) in v.ui.roles() {
        let _ = writeln!(out, "  {name} = \"{}\",", color.hex());
    }
    let mut table = |name: &str, colors: &mut dyn Iterator<Item = (&str, Color)>| {
        let _ = writeln!(out, "  {name} = {{");
        for (key, color) in colors {
            // In brackets: `function` is a word of the language.
            let _ = writeln!(out, "    [\"{key}\"] = \"{}\",", color.hex());
        }
        out.push_str("  },\n");
    };
    table("ansi", &mut ANSI_NAMES.into_iter().zip(v.ansi));
    table(
        "syntax",
        &mut SYNTAX_ROLES.into_iter().map(|r| (r, v.syntax_color(r))),
    );
    table(
        "diff",
        &mut DIFF_ROLES.into_iter().map(|r| (r, v.diff_color(r))),
    );
    table(
        "diagnostic",
        &mut DIAGNOSTIC_ROLES
            .into_iter()
            .map(|r| (r, v.diagnostic_color(r))),
    );
    out.push_str("}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundled;

    #[test]
    fn golden() {
        let themes = bundled::themes();
        let golden = [
            (
                include_str!("../tests/golden/aetherius.nvim.lua"),
                include_str!("../tests/golden/aetherius.nvim-palette.lua"),
            ),
            (
                include_str!("../tests/golden/ember.nvim.lua"),
                include_str!("../tests/golden/ember.nvim-palette.lua"),
            ),
        ];
        for (theme, (scheme, palette)) in themes.iter().zip(golden) {
            let theme = Resolved::new(theme, None).unwrap();
            assert_eq!(render(&theme), scheme);
            assert_eq!(render_palette(&theme), palette);
        }
    }

    /// No group is set twice, and a group that follows another follows one
    /// that exists here or in Neovim itself.
    #[test]
    fn groups_are_set_once() {
        let themes = bundled::themes();
        let theme = Resolved::new(&themes[0], None).unwrap();
        let mut names: Vec<&str> = groups(&theme).iter().map(|(n, _)| *n).collect();
        names.extend(LINKS.iter().map(|(n, _)| *n));
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count);
        for (_, to) in LINKS {
            assert!(names.contains(&to), "{to}");
        }
    }
}
