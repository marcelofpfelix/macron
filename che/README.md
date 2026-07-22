# che

`che` indexes local keybindings and opens them as a searchable cheatsheet.

Current sources:

- Hyprland Lua profiles in the dotfiles checkout
- tmux config
- Neovim Lua keymaps

## Commands

```sh
che list
che list vim
che find hyp
che vim              # fuzzy-find Neovim bindings
che mux              # fuzzy-find tmux bindings
che hyp              # fuzzy-find Hyprland bindings
che explain 'C-a h' mux
che explain Super+1 hyp
che conflicts
che dump --format json
che tui
che tui Super+1
che sheet vim        # render legacy desktop/che/vim.yml
```

`che` with no subcommand tries `fzf` and falls back to grouped text. The grouped view avoids one global table and shows local key/action rows under each app/profile/mode. `che tui` uses the shared Ratatui panel renderer for a compact read-only view.

Bindings can use comments immediately above a keymap, inline Lua comments, or Neovim `desc` values as the explanation shown by `che`.

Defaults and runtime Neovim maps are still future work; the current phase
indexes explicit local config.


## Dotfiles Name Collision

The dotfiles checkout currently has a shell cheatsheet wrapper at
`desktop/bin/che`. Installing this Rust binary as `che` is intentional for the
new keybinding index, but PATH order decides which command wins. The old YAML sheets remain available through `che sheet <name>`.
