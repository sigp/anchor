# Development Environment

Most Anchor developers work on Linux or MacOS, however Windows should still
be suitable.

First, follow the [`Installation Guide`](./installation.md) to install
Anchor. This will install Anchor to your `PATH`, which is not
particularly useful for development but still a good way to ensure you have the
base dependencies.

The additional requirements for developers are:

- [`docker`](https://www.docker.com/). Some tests need docker installed and **running**.

## Using `make`

Commands to run the test suite are available via the `Makefile` in the
project root for the benefit of CI/CD. We list some of these commands below so
you can run them locally and avoid CI failures:

- `$ make cargo-fmt`: (fast) runs a Rust code formatting check.
- `$ make lint`: (fast) runs a Rust code linter.
- `$ make test`: (medium) runs unit tests across the whole project.
- `$ make test-specs`: (medium) runs the Anchor test vectors.
- `$ make test-full`: (slow) runs the full test suite (including all previous
  commands). This is approximately everything
 that is required to pass CI.

## Testing

As with most other Rust projects, Anchor uses `cargo test` for unit and
integration tests. For example, to test the `qbft` crate run:

```bash
cd src/qbft
cargo test
```

## Local Testnets

During development and testing it can be useful to start a small, local
testnet.

Testnet scripts will be built as the project develops.
