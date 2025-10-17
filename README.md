# Anchor :anchor:
#### Secret Share Validator (SSV) Validator Client

[![Docs Status]][Docs Link] [![CI status]][gh-ci]

Anchor is an open source implementation of the Secret Shared Validator (SSV) protocol, written
in Rust and maintained by [Sigma Prime](https://github.com/sigp).

[CI Status]: https://github.com/sigp/anchor/workflows/test-suite/badge.svg
[gh-ci]: https://github.com/sigp/anchor/actions/workflows/test-suite.yml
[Docs Status]:https://img.shields.io/badge/user--docs-stable-informational
[Docs Link]: https://anchor.sigmaprime.io
[stable]: https://github.com/sigp/anchor/tree/stable
[unstable]: https://github.com/sigp/anchor/tree/unstable
[blog]: https://blog.sigmaprime.io

## Overview

This client implementation is currently under active development and should not
be used for production until a formal production release has been made.

## Documentation

The [Anchor Docs](https://anchor.sigmaprime.io) contains information for users and
developers. Instructions for how to compile/build and run this client are all
contained within this book.

## Branches

Anchor maintains two permanent branches:

- [`stable`][stable]: Always points to the latest stable release.
  - This is ideal for most users.
- [`unstable`][unstable]: Used for development, contains the latest PRs.
  - Developers should base their PRs on this branch.

## Metrics

Anchor has a suite of metrics that can be accessed via Prometheus and Grafana. See the
[metrics](https://github.com/sigp/anchor/tree/HEAD/metrics) page for more information and how to
setup.

## Contributing

Anchor welcomes contributors.

If you are looking to contribute, please head to the
[Contributing](https://anchor.sigmaprime.io/contributing) section
of the Anchor book.

## Contact

The best place to reach us in the
[#anchor](https://discord.com/channels/605577013327167508/1376460624069918720) channel in our [Lighthouse
discord server](https://discord.gg/cyAszAh).

For security related matters, please reach out to
[security@sigmaprime.io](mailto:security@sigmaprime.io) and encrypt sensitive
messages with our [PGP key](https://keybase.io/sigp/pgp_keys.asc?fingerprint=15e66d941f697e28f49381f426416dc3f30674b0).
