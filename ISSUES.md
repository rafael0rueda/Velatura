# Known issues to solve later

## Edits to the theme in use don't reach the desktop

**Found:** 2026-10-30, while writing the user manual.

**Status:** open.

### What happens

After you edit the theme in use and select **Save Theme**, the desktop keeps the
old colors, and the app offers no clear way to apply the new ones:

- The theme's card does nothing when selected, because the theme is in use.
- The **Applied** button on the theme's Details page is disabled.
- **Apply Changes** doesn't appear, because it only tracks parts being switched
  on or off.

Nothing tells the user that the saved colors aren't on the desktop yet.

### Steps to reproduce

1. Create a theme and apply it.
2. Open its card's **⋮** menu and select **Edit Theme**.
3. Change a color and select **Save Theme**.
4. Look for a way to apply the change from the card, the Details page or the
   top of the Themes view.

### Workaround

Open the card's **⋮** menu, select **Preview Changes…**, then select **Apply
Theme**.

In a sandbox, `velatura apply <theme> --preview` listed the updates after a
theme's file was edited, and `velatura apply <theme> --yes` wrote the new color.
The app's **Preview Changes…** uses the same planning code, but this path hasn't
been clicked through in the app.

### Where it is

- `crates/velatura-gui/src/main.rs`, `pending()`: only looks at the parts, not at
  whether the theme's colors have changed.
- `crates/velatura-gui/src/main.rs`, the card's `connect_clicked` in the theme
  card code: does nothing for the theme in use.
- `crates/velatura-gui/src/main.rs`, `open_theme()`: uses `pending()` to choose
  between **Apply Changes** and **Applied**.
- `crates/velatura-gui/src/editor.rs`, `save_current()`: saves the theme without
  applying it or saying that it still needs applying.

### Suggested fix

Make `pending()` also notice when the theme in use differs from what was last
applied, so the existing **Apply Changes** button appears on the Themes view and
the Details page. Saving could also say so in its message.

### When it's fixed

Remove the note "If the theme you edited is the one in use" from "Change a theme
later" in `docs/user-manual.md`, and remove this entry.
