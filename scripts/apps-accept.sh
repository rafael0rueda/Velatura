#!/usr/bin/env bash
# The apps themselves must accept what the renderers produce for them:
# tmux, Neovim and Starship each load the golden files in a throw-away home.
#
#   scripts/apps-accept.sh <golden folder>
#
# An app that is not installed is reported and passed over.
set -euo pipefail
golden=$(cd "$1" && pwd)
root=$(mktemp -d)
trap 'tmux_do kill-server >/dev/null 2>&1 || true; rm -rf "$root"' EXIT
mkdir -p "$root/home/.config" "$root/run"
sandbox=(env -i "PATH=$PATH" "HOME=$root/home" "XDG_CONFIG_HOME=$root/home/.config"
    "XDG_DATA_HOME=$root/home/.local/share" "XDG_STATE_HOME=$root/home/.local/state"
    "XDG_CACHE_HOME=$root/home/.cache" "TMUX_TMPDIR=$root/run" TERM=xterm-256color)
tmux_do() { "${sandbox[@]}" tmux -L velatura-accept "$@"; }

# The value a file gives a key: the text between the quotes on its line.
value() { grep -m1 -F -- "$2" "$1" | sed 's/.*"\(.*\)"$/\1/'; }

if command -v tmux >/dev/null 2>&1; then
    tmux_do -f /dev/null new-session -d -x 80 -y 24
    for f in "$golden"/*.tmux.conf; do
        tmux_do source-file "$f"
        want=$(value "$f" "set -g mode-style ")
        got=$(tmux_do show -gv mode-style)
        [ "$got" = "$want" ] || { echo "tmux shows mode-style $got, not $want"; exit 1; }
        echo "tmux accepts $f"
    done
    tmux_do kill-server
else
    echo "NOT RUN: tmux is not installed"
fi

if command -v nvim >/dev/null 2>&1; then
    for f in "$golden"/*.nvim.lua; do
        rt="$root/nvim"
        rm -rf "$rt"; mkdir -p "$rt/colors" "$rt/lua/velatura"
        cp "$f" "$rt/colors/velatura.lua"
        cp "${f%.nvim.lua}.nvim-palette.lua" "$rt/lua/velatura/palette.lua"
        # Loaded twice, as when a theme is applied again; then the colours
        # Neovim computes are compared with the file's.
        got=$("${sandbox[@]}" nvim --headless -u NONE -i NONE --cmd "set rtp^=$rt" \
            -c "colorscheme velatura" -c "colorscheme velatura" \
            -c 'lua local n = vim.api.nvim_get_hl(0, { name = "Normal" })
                local k = vim.api.nvim_get_hl(0, { name = "@keyword", link = false })
                local p = require("velatura.palette")
                io.stdout:write(string.format("%s #%06x #%06x #%06x %s %s [%s]\n", vim.g.colors_name,
                    n.fg, n.bg, k.fg, p.bg, p.syntax["keyword"], vim.v.errmsg))' \
            -c q 2>&1)
        fg=$(grep -m1 -F 'hl(0, "Normal",' "$f" | sed 's/.*fg = "\([^"]*\)".*/\1/')
        bg=$(grep -m1 -F 'hl(0, "Normal",' "$f" | sed 's/.*bg = "\([^"]*\)".*/\1/')
        kw=$(grep -m1 -F 'hl(0, "Statement",' "$f" | sed 's/.*fg = "\([^"]*\)".*/\1/')
        want="velatura $fg $bg $kw $bg $kw []"
        [ "$got" = "$want" ] || { echo "Neovim gives: $got"; echo "expected:     $want"; exit 1; }
        echo "Neovim accepts $f"
    done
else
    echo "NOT RUN: Neovim is not installed"
fi

if command -v starship >/dev/null 2>&1; then
    for f in "$golden"/*.starship.toml; do
        # Velatura's lines at the top of a file that has tables after them,
        # and a prompt that names two of the palette's colours.
        conf="$root/starship.toml"
        { cat "$f"; printf 'format = "[a](accent)[d](directory)"\n[palettes.other]\nx = "#000000"\n'; } > "$conf"
        out=$("${sandbox[@]}" "STARSHIP_CONFIG=$conf" STARSHIP_SHELL=sh starship prompt 2>&1)
        accent=$(value "$f" "palettes.velatura.accent ")
        rgb=$(printf '%d;%d;%d' "0x${accent:1:2}" "0x${accent:3:2}" "0x${accent:5:2}")
        case "$out" in
            *WARN* | *ERROR*) echo "Starship complains: $out"; exit 1 ;;
            *"38;2;$rgb"*) echo "Starship accepts $f" ;;
            *) echo "Starship does not show the accent $accent: $out"; exit 1 ;;
        esac
    done
else
    echo "NOT RUN: Starship is not installed"
fi
