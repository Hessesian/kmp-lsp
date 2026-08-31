-- Port of Helix's built-in `jetbrains_dark` theme to a Neovim colorscheme.
--
-- Source of truth: /usr/lib/helix/runtime/themes/jetbrains_dark.toml (palette
-- + scope table), read directly rather than re-derived from memory. This is
-- the base theme `jetbrains_kotlin.toml` (see docs/themes/ in the kmp-lsp
-- repo) inherits from — porting it gives Neovim the same background/UI chrome
-- Helix shows, so the `jetbrains-kotlin` semantic-token overlay (which only
-- sets accent colors, not background) has a matching base to sit on top of.

vim.cmd("hi clear")
if vim.fn.exists("syntax_on") == 1 then
  vim.cmd("syntax reset")
end
vim.o.termguicolors = true
vim.o.background = "dark"
vim.g.colors_name = "jetbrains_dark"

-- palette (hex values copied from jetbrains_dark.toml's [palette] table)
local p = {
  red179 = "#b3ae60",
  red194 = "#c29e4a",
  red199 = "#c77dbb",
  red207 = "#cf8e6d",
  red213 = "#d5b778",
  red214 = "#d64d5b",

  green130 = "#5f826b",
  green145 = "#549159",
  green156 = "#499c54",
  green171 = "#6aab73",

  blue34  = "#1e1f22",
  blue46  = "#26282e",
  blue48  = "#2b2d30",
  blue50  = "#2d2e32",
  blue56  = "#313438",
  blue64  = "#293c40",
  blue81  = "#4d4e51",
  blue89  = "#4b5059",
  blue110 = "#2e436e",
  blue122 = "#6f737a",
  blue131 = "#214283",
  blue133 = "#7a7e85",
  blue145 = "#868a91",
  blue171 = "#A1A3AB",
  blue173 = "#375fad",
  blue184 = "#2aacb8",
  blue196 = "#bcbec4",
  blue245 = "#56a8f5",
  blue247 = "#57aaf7",
}

local hl = vim.api.nvim_set_hl

-- ── UI (from "ui.*" scopes) ──────────────────────────────────────────────
hl(0, "Normal",        { fg = p.blue196, bg = p.blue34 })
hl(0, "NormalNC",      { fg = p.blue196, bg = p.blue34 })
hl(0, "NormalFloat",   { fg = p.blue196, bg = p.blue48 })
hl(0, "FloatBorder",   { fg = p.blue89,  bg = p.blue48 })
hl(0, "SignColumn",    { bg = p.blue34 })
hl(0, "LineNr",        { fg = p.blue56 })
hl(0, "CursorLineNr",  { fg = p.blue171 })
hl(0, "CursorLine",    { bg = p.blue46 })
hl(0, "CursorColumn",  { bg = p.blue46 })
hl(0, "ColorColumn",   { bg = p.blue46 })
hl(0, "VertSplit",     { fg = p.blue56, bg = p.blue34 })
hl(0, "WinSeparator",  { fg = p.blue56, bg = p.blue34 })
hl(0, "StatusLine",    { fg = p.blue196, bg = p.blue48 })
hl(0, "StatusLineNC",  { fg = p.blue89,  bg = p.blue48 })
hl(0, "TabLine",       { fg = p.blue89,  bg = p.blue48 })
hl(0, "TabLineSel",    { fg = p.blue196, bg = p.blue34 })
hl(0, "TabLineFill",   { bg = p.blue48 })
hl(0, "Pmenu",         { fg = p.blue196, bg = p.blue48 })
hl(0, "PmenuSel",      { fg = p.blue196, bg = p.blue110 })
hl(0, "PmenuSbar",     { bg = p.blue48 })
hl(0, "PmenuThumb",    { bg = p.blue81 })
hl(0, "Visual",        { bg = p.blue131 })
hl(0, "VisualNOS",     { bg = p.blue131 })
hl(0, "Search",        { fg = p.blue34, bg = p.red194 })
hl(0, "IncSearch",     { fg = p.blue34, bg = p.blue247 })
hl(0, "CurSearch",     { fg = p.blue34, bg = p.blue247 })
hl(0, "MatchParen",    { bg = p.blue110, bold = true })
hl(0, "NonText",       { fg = p.blue122 })
hl(0, "Whitespace",    { fg = p.blue122 })
hl(0, "EndOfBuffer",   { fg = p.blue34 })
hl(0, "Title",         { fg = p.blue196, bold = true })
hl(0, "Directory",     { fg = p.blue245 })
hl(0, "Cursor",        { fg = p.blue34, bg = p.blue196 })

-- ── Diagnostics ("warning"/"error"/"info"/"hint") ───────────────────────
hl(0, "DiagnosticError", { fg = p.red214 })
hl(0, "DiagnosticWarn",  { fg = p.red194 })
hl(0, "DiagnosticInfo",  { fg = p.blue247 })
hl(0, "DiagnosticHint",  { fg = p.green156 })
hl(0, "DiagnosticUnderlineError", { sp = p.red214, undercurl = true })
hl(0, "DiagnosticUnderlineWarn",  { sp = p.red194, undercurl = true })
hl(0, "DiagnosticUnderlineInfo",  { sp = p.blue247, undercurl = true })
hl(0, "DiagnosticUnderlineHint",  { sp = p.green156, undercurl = true })

-- ── Syntax (legacy groups, from the scope table) ────────────────────────
hl(0, "Comment",        { fg = p.blue133, italic = true })
hl(0, "SpecialComment", { fg = p.green130 })
hl(0, "String",         { fg = p.green171 })
hl(0, "Character",      { fg = p.green171 })
hl(0, "Number",         { fg = p.blue184 })
hl(0, "Float",          { fg = p.blue184 })
hl(0, "Boolean",        { fg = p.red207 })
hl(0, "Constant",       { fg = p.red207 })
hl(0, "Identifier",     { fg = p.blue196 })
hl(0, "Function",       { fg = p.blue245 })
hl(0, "Macro",          { fg = p.red179 })
hl(0, "Statement",      { fg = p.red207 })
hl(0, "Conditional",    { fg = p.red207 })
hl(0, "Repeat",         { fg = p.red207 })
hl(0, "Label",          { fg = p.red199 })
hl(0, "Operator",       { fg = p.blue196 })
hl(0, "Keyword",        { fg = p.red207 })
hl(0, "Exception",      { fg = p.red207 })
hl(0, "PreProc",        { fg = p.red207 })
hl(0, "Include",        { fg = p.red207 })
hl(0, "Define",         { fg = p.red207 })
hl(0, "PreCondit",      { fg = p.red207 })
hl(0, "Type",           { fg = p.blue196 })
hl(0, "StorageClass",   { fg = p.red207 })
hl(0, "Structure",      { fg = p.blue196 })
hl(0, "Typedef",        { fg = p.blue196 })
hl(0, "Special",        { fg = p.red199 })
hl(0, "SpecialChar",    { fg = p.red199 })
hl(0, "Tag",            { fg = p.red213 })
hl(0, "Delimiter",      { fg = p.blue196 })
hl(0, "Underlined",     { fg = p.blue247, underline = true })
hl(0, "Error",          { fg = p.red214 })
hl(0, "Todo",           { fg = p.blue34, bg = p.red194, bold = true })

-- ── Treesitter fallback groups (harmless even without nvim-treesitter
-- installed; keeps things consistent if it's added later) ──────────────
local ts_links = {
  ["@variable"]          = "Identifier",
  ["@variable.builtin"]  = "Boolean",
  ["@variable.member"]   = "Label",
  ["@parameter"]         = "Identifier",
  ["@function"]          = "Function",
  ["@function.macro"]    = "Macro",
  ["@keyword"]            = "Keyword",
  ["@keyword.function"]   = "Keyword",
  ["@keyword.return"]     = "Keyword",
  ["@type"]                = "Type",
  ["@type.builtin"]        = "Boolean",
  ["@constant"]             = "Constant",
  ["@constant.builtin"]    = "Boolean",
  ["@number"]               = "Number",
  ["@string"]               = "String",
  ["@comment"]              = "Comment",
  ["@tag"]                  = "Tag",
  ["@punctuation.delimiter"] = "Delimiter",
  ["@operator"]              = "Operator",
  ["@markup.link"]           = "Underlined",
}
for from, to in pairs(ts_links) do
  hl(0, from, { link = to })
end
