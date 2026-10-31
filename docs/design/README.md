# Design

Kiri's interface is the terminal, and the repository uses no design tool. A screen is documented as a
mockup of the terminal in a monospace block, then as a capture of the real interface once `kiri-tui` runs.

| File | Holds |
| --- | --- |
| [brand/](brand/README.md) | The mark, what it means, and every brand asset |
| [tokens.md](tokens.md) | The palette, in the terminal and on the docs site |

`pages/` does not exist yet: no screen has been specced. Each screen goes in `pages/<feature>/<screen>.md`,
and its feature's spec links it.
