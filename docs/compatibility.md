# Versioning & Compatibility Policy

## Versioning

Fila uses [semantic versioning](https://semver.org/) (semver) for all releases:

- **MAJOR** — Breaking proto/API changes. Existing SDK versions may not work.
- **MINOR** — New features, backward compatible. Existing SDKs continue to work; new features require SDK update.
- **PATCH** — Bug fixes only. No behavior changes.

## Proto Backward Compatibility

The `fila.v1` proto package follows strict additive-only rules within a MAJOR version:

- New RPCs may be added
- New fields may be added to existing messages
- Existing fields are never removed, renamed, or retyped
- Field numbers are never reused

A MAJOR version bump (e.g., v1 → v2) would introduce a new proto package (`fila.v2`) and may remove deprecated RPCs or fields.

## Deprecation Policy

- Features are deprecated with at least 1 MINOR version warning before removal
- Deprecated RPCs and fields are documented in release notes and proto comments
- Removal only occurs in a MAJOR version bump
