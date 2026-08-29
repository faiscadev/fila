# Versioning & Compatibility Policy

## Release versioning

Fila uses [semantic versioning](https://semver.org/):

- **MAJOR** — Breaking changes to the wire protocol or the SDK surface. Existing
  clients may stop working.
- **MINOR** — New features, backward compatible. Existing clients keep working;
  using the new features requires an SDK update.
- **PATCH** — Bug fixes only. No behavior changes.

The broker's release version and the protocol version are independent. A MINOR
broker release may add a protocol capability without changing the protocol version.

## Protocol compatibility

The wire protocol carries its own version, negotiated during the handshake. See
[protocol.md](protocol.md) for the mechanism.

Within a protocol version:

- New fields may be appended to the end of an opcode body
- Existing fields are never removed, reordered, or retyped
- New opcodes may be added
- Readers must tolerate trailing bytes they do not recognize

Changing field order or type within an opcode requires a protocol version bump.

### Prefer capabilities over version bumps

A protocol version bump is for **layout** changes. Behaviour that either side may or
may not implement belongs behind a capability bit negotiated in the handshake, so it
ships without breaking older clients.

Adding a capability is always backward compatible: a peer that does not advertise
the bit simply does not get the feature.

## Deprecation policy

- Features are deprecated with at least one MINOR release of warning before removal
- Deprecated fields and opcodes are documented in release notes and in
  [protocol.md](protocol.md)
- Removal happens only in a MAJOR release
- Deprecated protocol fields are never deleted from an existing opcode body; they
  carry zero or empty values until the version that removes them
