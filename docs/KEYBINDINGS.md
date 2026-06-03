# Keybindings

## Panes
| Action | Keys |
|---|---|
| Split right (left/right) | `Ctrl+Shift+D` |
| Split down (top/bottom) | `Ctrl+Shift+E` |
| Move focus | `Ctrl+Shift+←↑↓→` |
| Resize focused pane | `Ctrl+Alt+←↑↓→`, `Ctrl+Mouse-wheel`, or drag the divider |
| Rename pane | `Ctrl+Shift+R` |
| Close pane | `Ctrl+Shift+W` (or type `exit`) |
| Font zoom in / out / reset | `Ctrl +` / `Ctrl -` / `Ctrl 0` |

## Tabs
| Action | Keys |
|---|---|
| New tab | `Ctrl+Shift+T` (or click `+` in the tab strip) |
| Next tab | `Ctrl+Tab` |
| Jump to tab N | `Ctrl+1` … `Ctrl+9` |
| Switch / close (mouse) | Click a tab / click its `×` |
| Move tab to a new window | `Ctrl+Shift+N`, or **drag a tab off the bar** and drop it outside the window |

## Clipboard & scrollback
| Action | Keys |
|---|---|
| Copy | `Ctrl+Shift+C` (or drag-select — auto-copies) |
| Paste | `Ctrl+Shift+V`, or right-click (sanitized, bracketed-paste safe) |
| Broadcast paste | `Ctrl+Shift+B` — paste the clipboard into **every pane in every window** at once (sanitized, per-pane bracketed-paste safe) |
| Scroll | Mouse wheel (typing snaps back to the bottom) |
| Open hyperlink | `Ctrl+Click` an OSC-8 link (http/https/file only) |

## Overlays & modes
| Action | Keys |
|---|---|
| Command palette | `Ctrl+Shift+P` (fuzzy; ↑/↓, Enter, Esc) |
| Keybinding help | `F1` or `Ctrl+Shift+/` |
| Broadcast / seamless mode | via the command palette |

## Command palette
Fuzzy-searchable: split right/down, close pane, rename pane, new tab,
next/previous tab, toggle broadcast input, toggle seamless mode, move tab to new
window, save session, load session.

## Multiple windows
Tear a tab into its own window — drag it off the bar onto another monitor, or
press `Ctrl+Shift+N`. The torn tab keeps its live panes and running shells. Each
window has its own tabs, focus, and broadcast/seamless state; keystrokes only go
to the focused window. A restart reopens every window where you left it.

## Broadcast mode
Types into every pane in the tab at once. Auto-disables on tab switch; a
signal-bearing control byte (`Ctrl+C`/`Ctrl+D`/`Ctrl+Z`) needs a confirming
second press so a stray interrupt can't hit every pane.

## Mouse summary
Drag a divider to resize · click a tab (or its `×`/`+`) · **drag a tab off the
bar to tear it into a new window** · drag to select+copy · right-click to paste ·
click a pane to focus · `Ctrl+Click` a link to open it · when a TUI app enables
mouse mode, clicks/drag/wheel are forwarded to it.
