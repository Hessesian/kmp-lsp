-- JetBrains Darcula accents for kmp-lsp's semantic tokens.
--
-- Unlike Helix, Neovim renders LSP semantic tokens directly (as `@lsp.type.*`
-- / `@lsp.mod.*` highlight groups) instead of requiring a tree-sitter query
-- to carry the same information. So this is a straight port of the palette
-- from docs/themes/jetbrains_kotlin.toml in the kmp-lsp repo — no query file
-- equivalent needed.
--
-- Applies on top of whatever colorscheme is active. Call M.apply() once at
-- startup and again on the `ColorScheme` autocmd so it survives `:colorscheme`
-- switches (colorscheme changes reset highlight groups first).

local M = {}

-- IntelliJ Darcula exact colors (same values as jetbrains_kotlin.toml's [palette])
local palette = {
  periwinkle_param  = "#94BBFF", -- value parameters (declaration + use-sites)
  yellow_annotation = "#BBB529", -- @annotation / decorator
  gold_method       = "#FFC66D", -- function / method names
  purple_field      = "#9876AA", -- fields, properties, enum entries
  teal_type_param   = "#20999D", -- type parameters <T>
  default_text      = "#A9B7C6", -- default identifier text
  orange_keyword    = "#CC7832", -- keywords
}

function M.apply()
  local hl = vim.api.nvim_set_hl

  -- LSP semantic token types kmp-lsp emits (src/semantic_tokens/mod.rs TOKEN_TYPES),
  -- mapped 1:1 to Neovim's `@lsp.type.<type>` groups.
  hl(0, "@lsp.type.parameter",     { fg = palette.periwinkle_param })
  hl(0, "@lsp.type.decorator",     { fg = palette.yellow_annotation })
  hl(0, "@lsp.type.function",      { fg = palette.gold_method })
  hl(0, "@lsp.type.method",        { fg = palette.gold_method })
  hl(0, "@lsp.type.property",      { fg = palette.purple_field })
  hl(0, "@lsp.type.enumMember",    { fg = palette.purple_field })
  hl(0, "@lsp.type.enum",          { fg = palette.purple_field })
  hl(0, "@lsp.type.typeParameter", { fg = palette.teal_type_param })
  hl(0, "@lsp.type.class",         { fg = palette.default_text })
  hl(0, "@lsp.type.interface",     { fg = palette.default_text })
  hl(0, "@lsp.type.struct",        { fg = palette.default_text })
  hl(0, "@lsp.type.variable",      { fg = palette.default_text })
  hl(0, "@lsp.type.namespace",     { fg = palette.default_text })
  hl(0, "@lsp.type.operator",      { fg = palette.default_text })
  hl(0, "@lsp.type.keyword",       { fg = palette.orange_keyword })

  -- STATIC modifier (companion object members) — IntelliJ marks these italic.
  -- `@lsp.mod.*` overlays regardless of the token's base type.
  hl(0, "@lsp.mod.static", { italic = true })
end

vim.api.nvim_create_autocmd("ColorScheme", { callback = M.apply })

return M
