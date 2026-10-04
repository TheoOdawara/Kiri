# Kiri — documentation

| Folder | Holds |
| --- | --- |
| [requirements/](requirements/README.md) | The versioned SRS: scope, milestones, and every requirement |
| [architecture/](architecture/README.md) | The map: building blocks, runtime scenarios, deployment, concepts, risks |
| [data-model/](data-model/README.md) | The conceptual model: entities and their relations |
| [diagrams/](diagrams/README.md) | The `.drawio.svg` files the other docs embed; edited with the draw.io extension |
| [decisions/](decisions/0001-architecture.md) | ADRs: why each cross-cutting choice was made |
| [brand/](brand/branding.md) | The brand: logo, seal, palette |

- `roadmap/` does not exist yet: the first sprint plan creates it.
- `specs/` does not exist yet: no feature has been specced.

## Reading it as a site

From the repository root, run the `docs: serve` task in VS Code, or:

```
uvx zensical==0.0.67 serve --open
```

It serves on `http://localhost:8000`. The site is local only and is never deployed.
