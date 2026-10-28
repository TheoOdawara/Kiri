# Design tokens

Kiri's interface is a terminal, so its tokens are colors only: type is the terminal's own font, and
spacing, radius, and shadow do not exist there. The palette is "Tamahagane Void", the starting point the
interface's spec redesigns ([FR-UI-07](../requirements/functional/interfaces.md#fr-ui-07)); that spec
updates this file. The app's own source of these values does not exist yet, because `kiri-tui` is not
built.

## Terminal

Dark only. The ANSI column is the approximate SGR code for a terminal without truecolor
([NFR-INTR-01](../requirements/non-functional/interaction.md#nfr-intr-01)).

| Token | Value | ANSI | Use |
| --- | --- | --- | --- |
| `background` | `#0D1117` | 40 | The base of the interface |
| `foreground` | `#E6EDF3` | 37 | Body text |
| `brand` | `#8B949E` | 90 | Neutral harness elements, borders, delimiters |
| `success` | `#3FB950` | 32 | Gate passed: checks green, permission granted |
| `warning` | `#D29922` | 33 | Gate warning: a performance or build warning |
| `error` | `#F85149` | 31 | Gate blocked: a security failure, code the harness stopped |
| `highlight` | `#58A6FF` | 36 | Selection, and orchestration in progress |

## Docs site

[`docs-theme.css`](../stylesheets/docs-theme.css) carries these values. The terminal has no light palette,
so the light scheme uses the background as its primary color and darkens the highlight to read on white.
Values not in the table above are derived from it. Zensical's modern theme keeps its translucent header and
applies the primary color to active navigation.

| Variable | Light | Dark |
| --- | --- | --- |
| Primary | `#0D1117` | `#161B22` |
| Text on primary | `#E6EDF3` | `#E6EDF3` |
| Links and accent | `#0969DA` | `#58A6FF` |
| Page background | Zensical's default | `#0D1117` |
