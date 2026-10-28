# Kiri — documentation

| Folder | Holds |
| --- | --- |
| [requirements/](requirements/README.md) | The versioned SRS: scope, milestones, and every requirement |
| [architecture/](architecture/README.md) | The map: building blocks, runtime scenarios, deployment, concepts, risks |
| [data-model/](data-model/README.md) | The conceptual model: entities and their relations |
| [decisions/](decisions/README.md) | ADRs: why each cross-cutting choice was made |
| [design/](design/README.md) | The brand, the tokens, and each screen of the terminal interface |

- Each doc keeps its diagrams in its own `diagrams/` folder, as `.drawio.svg` files the draw.io extension
  edits in VS Code.
- `roadmap/` does not exist yet: the first sprint plan creates it.
- `specs/` does not exist yet: no feature has been specced.

## Reading it as a site

From the repository root, run the `docs: serve` task in VS Code, or:

```
uvx zensical==0.0.73 serve --open
```

It serves on `http://localhost:8000`. The site is local only and is never deployed.
