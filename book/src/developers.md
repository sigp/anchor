# For Protocol Developers

_Documentation for protocol developers._

This section lists Anchor-specific decisions that are not strictly spec'd and may be useful for
other protocol developers wishing to interact with Anchor.

## SSV NodeInfo Handshake Protocol

The protocol is used by SSV-based nodes to exchange basic node metadata and validate each other's identity when establishing a connection over Libp2p under a dedicated protocol ID.  The spec is define here in the [SSV NodeInfo Handshake Protocol](./handshake.md).
