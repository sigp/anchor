//! SSV-specific validation of `builder_definitions.yml`, run once at startup after
//! Lighthouse's `BuilderStore` has loaded and validated the same file.
//!
//! Lighthouse validates against the beacon-API wire bounds (64 enabled entries, URL
//! shape and length, duplicate `(url, auth_data)` pairs), but it is SSV-unaware. Two
//! constraints come from the SSV protocol instead and are enforced here:
//!
//! - SIP-94 §5 caps configured builder entries at 8 per validator. Excess entries load fine but
//!   their request-auth signing roots exceed the gossip root budget peers enforce, so which
//!   builders survive depends on per-peer message arrival order: nondeterministic, silent
//!   sub-quorum drops. Failing startup is the only loud signal.
//! - A zero-length auth `data` is invalid per SIP-94 §5 and rejected by go-ssv at config load.
//!   Lighthouse instead drops such a builder with an error log at every produce. Rejecting it at
//!   startup keeps a config that go-ssv operators cannot even load from silently half-working on an
//!   Anchor operator.
//!
//! `BuilderStore`'s container type is crate-private, so the file is re-read here with a
//! minimal wrapper over the exported [`BuilderDefinition`] and the same `yaml_serde`
//! parser Lighthouse uses, keeping per-entry semantics and YAML dialect identical. The
//! filename coupling and the wrapper's field layout are pinned by the round-trip test
//! below, which writes through the real `BuilderStore` and asserts the wrapper sees the
//! entry.
//!
//! Enforcement boundary: this pass validates the file bytes once at startup, while the
//! store serves its own earlier read for the rest of the process lifetime. The two reads
//! open a moment in which an external writer could hand each different bytes, either
//! loudly (a parse failure here) or quietly (this pass approving a file the store never
//! loaded). No Anchor code writes the file, nothing mutates the store after load
//! (`insert` has no production caller), and edits apply on restart, so the window is
//! accepted rather than closed. A future Lighthouse accessor over the store's LOADED
//! definitions would close it and delete the wrapper; the re-read would still be the only
//! view of disabled entries under a `builder_config`-based inspection, which filters them
//! before signing. Checks run per entry first (most actionable), then the cap; the first
//! violation wins.

use std::{fs::File, path::Path};

use builder_store::{BuilderDefinition, BuilderStore};
use serde::Deserialize;

/// SIP-94 §5: "SSV caps configured entries at `8` per validator, a sub-cap of the
/// beacon-API's `MAX_BUILDER_ENTRIES` (64)."
///
/// Deliberately distinct from `message_validator`'s `MAX_REQUEST_AUTH_DISTINCT_ROOTS`:
/// this bounds configured entries, that bounds observed signing roots per proposal slot.
/// The SIP sets both to 8 (entries sharing auth `data` share a root, so the entry cap
/// implies the root cap), but neither derives from the other.
pub const MAX_SSV_BUILDER_ENTRIES: usize = 8;

/// The file `BuilderStore` reads. The name is fixed inside Lighthouse's `builder_store`
/// crate but not exported; the round-trip test pins the coupling.
const BUILDER_DEFINITIONS_FILENAME: &str = "builder_definitions.yml";

/// The subset of Lighthouse's (crate-private) builder config file this validation needs.
///
/// `default` keeps every file Lighthouse accepts acceptable here, `{}` included; format
/// drift on a future pin bump is caught by the round-trip test rather than by rejecting
/// files Lighthouse allows.
#[derive(Deserialize)]
struct BuilderDefinitionsFile {
    #[serde(default)]
    builders: Vec<BuilderDefinition>,
}

/// A failure opening the builder definitions store or a violation of the SSV
/// builder-config constraints.
///
/// The SSV-specific variants carry only entry indices and counts, never URLs or auth
/// bytes: builder URLs may embed credentials, and auth `data` must stay out of logs
/// entirely. `Store` re-surfaces Lighthouse's own error, which names URLs exactly as
/// Lighthouse's validator client does at load; auth bytes never appear in either.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Lighthouse's `BuilderStore` could not open, create, or validate the file.
    #[error("unable to open or create the builder definitions store: {0:?}")]
    Store(builder_store::Error),
    /// The definitions file could not be opened. `BuilderStore` creates it before this
    /// check runs, so this indicates a filesystem-level problem, not a missing file.
    #[error("unable to read the definitions file: {0}")]
    UnableToRead(std::io::Error),
    /// The definitions file could not be parsed. `BuilderStore` parsed the same bytes
    /// moments earlier, so this indicates either an external write racing startup or
    /// wrapper drift against a new Lighthouse pin.
    #[error("unable to parse the definitions file: {0}")]
    UnableToParse(yaml_serde::Error),
    /// More than [`MAX_SSV_BUILDER_ENTRIES`] enabled entries.
    #[error("{enabled} enabled builder entries exceed the SSV cap of {max} (SIP-94)")]
    TooManyEnabledEntries { enabled: usize, max: usize },
    /// The entry at `index` (file order, zero-based) resolves to zero-length auth
    /// `data`: either an explicit empty value (`auth_data: "0x"`) or an empty URL on a
    /// disabled entry. Omit `auth_data` to default to the URL bytes.
    #[error(
        "builder entry {index} resolves to zero-length auth data; omit `auth_data` to \
         default to the URL bytes"
    )]
    ZeroLengthAuthData { index: usize },
}

/// Open (or create) `<dir>/builder_definitions.yml` through Lighthouse's `BuilderStore`,
/// then enforce the SSV-specific constraints on the same file, returning the store for
/// block-service wiring.
///
/// The single entry point keeps the ordering structural: Lighthouse's own load
/// validation always runs first (it also creates the file on first start), so the SSV
/// pass never sees a file Lighthouse has not just vetted.
pub fn open_and_validate(builder_definitions_dir: &Path) -> Result<BuilderStore, Error> {
    let store = BuilderStore::open_or_create(builder_definitions_dir).map_err(Error::Store)?;
    validate_ssv_builder_constraints(builder_definitions_dir)?;
    Ok(store)
}

/// Enforce the SSV-specific builder-config constraints on `<dir>/builder_definitions.yml`.
///
/// The zero-length check resolves auth `data` exactly the way `builder_config` and
/// go-ssv do (explicit bytes, else `BuilderUrl::to_default_auth_data`) and covers ALL
/// entries, disabled ones included, matching go-ssv, which has no disabled concept and
/// validates everything. The entry cap counts ENABLED entries only: disabled entries
/// never reach the wire, and Lighthouse's own 64-entry wire cap also counts enabled only.
fn validate_ssv_builder_constraints(builder_definitions_dir: &Path) -> Result<(), Error> {
    let path = builder_definitions_dir.join(BUILDER_DEFINITIONS_FILENAME);
    let file = File::open(&path).map_err(Error::UnableToRead)?;
    let config: BuilderDefinitionsFile =
        yaml_serde::from_reader(file).map_err(Error::UnableToParse)?;

    for (index, definition) in config.builders.iter().enumerate() {
        let resolved_auth_data_len = match &definition.auth_data {
            Some(auth_data) => auth_data.len(),
            None => definition.url.to_default_auth_data().len(),
        };
        if resolved_auth_data_len == 0 {
            return Err(Error::ZeroLengthAuthData { index });
        }
    }

    let enabled = config
        .builders
        .iter()
        .filter(|definition| definition.enabled)
        .count();
    if enabled > MAX_SSV_BUILDER_ENTRIES {
        return Err(Error::TooManyEnabledEntries {
            enabled,
            max: MAX_SSV_BUILDER_ENTRIES,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bls::Signature;
    use builder_types::{RequestAuth, RequestAuthData, SignedRequestAuth};
    use parking_lot::Mutex;
    use tempfile::TempDir;
    use types::Slot;

    use super::*;

    /// The canonical fixture URL. Also the exact byte string the omitted-auth derivation
    /// vector asserts on, so keep it free of trailing slashes; the trailing-slash test adds
    /// its own variant.
    const TEST_URL: &str = "https://builder.example.com";

    /// Builds a definition with the given enabled flag, URL, and optional explicit auth
    /// data. The remaining fields are irrelevant to the SSV constraints and use minimal
    /// valid values.
    fn definition(enabled: bool, url: &str, auth_data: Option<Vec<u8>>) -> BuilderDefinition {
        BuilderDefinition {
            enabled,
            url: url.parse().expect("fixture URL fits the ByteList limit"),
            auth_data: auth_data
                .map(|bytes| RequestAuthData::new(bytes).expect("auth data fits the limit")),
            builder_pubkeys: vec![],
            max_execution_payment: 1,
            min_bid: None,
            builder_boost_factor: None,
        }
    }

    /// Opens a real `BuilderStore` in a fresh tempdir and inserts every definition through
    /// it, so the file under test carries exactly the bytes Lighthouse itself writes
    /// (filename, YAML dialect, and field encoding included). Returns the tempdir with the
    /// store so it outlives the test body.
    fn store_with(definitions: Vec<BuilderDefinition>) -> (TempDir, BuilderStore) {
        let dir = TempDir::new().expect("tempdir");
        let store = BuilderStore::open_or_create(dir.path()).expect("store should open");
        for definition in definitions {
            store
                .insert(definition)
                .expect("BuilderStore should accept the fixture entry");
        }
        (dir, store)
    }

    /// A distinct enabled-entry URL per index; the store rejects duplicate `(url, auth)`
    /// pairs, so multi-entry fixtures must vary the URL.
    fn indexed_url(index: usize) -> String {
        format!("https://builder{index}.example.com")
    }

    /// Asserts that `result` is `TooManyEnabledEntries` carrying `expected_enabled` and
    /// the SSV cap.
    fn expect_too_many_enabled(result: Result<(), Error>, expected_enabled: usize) {
        match result {
            Err(Error::TooManyEnabledEntries { enabled, max }) => {
                assert_eq!(
                    enabled, expected_enabled,
                    "the error should carry the enabled count"
                );
                assert_eq!(
                    max, MAX_SSV_BUILDER_ENTRIES,
                    "the error should carry the SSV cap"
                );
            }
            other => panic!("expected TooManyEnabledEntries, got: {other:?}"),
        }
    }

    /// Drives `BuilderStore::builder_config` on the store with a closure that records every
    /// auth `data` it is asked to sign, returning the recorded byte strings SORTED. The
    /// store may invoke the closure concurrently and in unspecified order, so callers must
    /// never assert on callback order; sorting makes that structural.
    async fn captured_auth_data(store: &BuilderStore) -> Vec<Vec<u8>> {
        let captured: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let capture = captured.clone();
        store
            .builder_config(move |data: RequestAuthData| {
                let capture = capture.clone();
                async move {
                    capture.lock().push(data.to_vec());
                    Ok::<_, std::convert::Infallible>(SignedRequestAuth {
                        message: RequestAuth {
                            data,
                            slot: Slot::new(0),
                        },
                        signature: Signature::empty(),
                    })
                }
            })
            .await;
        let mut data = std::mem::take(&mut *captured.lock());
        data.sort();
        data
    }

    // ==================== Entry-cap tests ====================

    /// One enabled entry over the SSV cap fails with the exact count and cap, even though
    /// Lighthouse's own store (64-entry cap) accepts the file.
    #[test]
    fn nine_enabled_entries_fail() {
        // Arrange
        let over_cap = MAX_SSV_BUILDER_ENTRIES + 1;
        let (dir, _) = store_with(
            (0..over_cap)
                .map(|i| definition(true, &indexed_url(i), None))
                .collect(),
        );

        // Act
        let result = validate_ssv_builder_constraints(dir.path());

        // Assert
        expect_too_many_enabled(result, over_cap);
    }

    /// Exactly the cap of enabled entries passes, and disabled entries do not count toward
    /// it: they never reach the wire, matching Lighthouse's own enabled-only cap semantics.
    #[test]
    fn eight_enabled_plus_disabled_entries_pass() {
        // Arrange
        let mut definitions: Vec<BuilderDefinition> = (0..MAX_SSV_BUILDER_ENTRIES)
            .map(|i| definition(true, &indexed_url(i), None))
            .collect();
        definitions.push(definition(false, "https://disabled0.example.com", None));
        definitions.push(definition(false, "https://disabled1.example.com", None));
        let (dir, _) = store_with(definitions);

        // Act
        let result = validate_ssv_builder_constraints(dir.path());

        // Assert
        assert!(
            result.is_ok(),
            "{MAX_SSV_BUILDER_ENTRIES} enabled plus disabled entries should pass, got: \
             {result:?}"
        );
    }

    // ==================== Zero-length auth tests ====================

    /// A DISABLED entry with explicit empty auth data (`auth_data: "0x"`) fails with the
    /// entry's file-order index. This is the constraint Lighthouse does not own: its
    /// `validate()` skips disabled entries entirely (which is also why the arrange step can
    /// write the entry through the real store's `insert`), while go-ssv, with no disabled
    /// concept, rejects the config outright. If a future pin makes the `insert` here fail,
    /// Lighthouse has started validating disabled entries and this check may be redundant.
    #[test]
    fn disabled_zero_length_auth_data_fails() {
        // Arrange: explicit empty auth data (`auth_data: "0x"`).
        let (dir, _) = store_with(vec![
            definition(true, TEST_URL, None),
            definition(false, "https://disabled.example.com", Some(vec![])),
        ]);

        // Act + Assert
        match validate_ssv_builder_constraints(dir.path()) {
            Err(Error::ZeroLengthAuthData { index }) => {
                assert_eq!(
                    index, 1,
                    "the error should carry the file-order entry index"
                );
            }
            other => panic!("expected ZeroLengthAuthData, got: {other:?}"),
        }

        // Arrange: an EMPTY URL on a disabled entry resolves to zero-length DEFAULT auth
        // data (the `None` branch of the resolution; an enabled empty URL dies in
        // Lighthouse's own URL validation first, so only disabled entries reach it).
        let (dir, _) = store_with(vec![
            definition(true, TEST_URL, None),
            definition(false, "", None),
        ]);

        // Act + Assert
        match validate_ssv_builder_constraints(dir.path()) {
            Err(Error::ZeroLengthAuthData { index }) => {
                assert_eq!(index, 1, "the empty-URL default must also be caught");
            }
            other => {
                panic!("expected ZeroLengthAuthData for the empty-URL default, got: {other:?}")
            }
        }
    }

    // ==================== Round-trip / drift-guard test ====================

    /// Drift guard for the crate-private wrapper: a file written entirely through the real
    /// `BuilderStore` passes the check, and the check demonstrably SEES the store's entries
    /// rather than passing vacuously. If a future Lighthouse pin renamed the `builders` key
    /// or the filename, the wrapper (with its `#[serde(default)]`) would silently parse
    /// zero entries and every constraint would pass forever; the over-cap phase here would
    /// then fail loudly instead.
    #[test]
    fn wrapper_round_trip_via_builder_store() {
        // Arrange: one entry through the real store.
        let (dir, store) = store_with(vec![definition(true, &indexed_url(0), None)]);

        // Act + Assert: a store-written single-entry file passes.
        let result = validate_ssv_builder_constraints(dir.path());
        assert!(
            result.is_ok(),
            "a store-written single-entry file should pass, got: {result:?}"
        );

        // Arrange: push the same store past the SSV cap (Lighthouse's own cap is 64, so
        // every insert succeeds).
        let over_cap = MAX_SSV_BUILDER_ENTRIES + 1;
        for i in 1..over_cap {
            store
                .insert(definition(true, &indexed_url(i), None))
                .expect("the store accepts up to its own 64-entry cap");
        }

        // Act + Assert: the check now counts exactly the inserted entries, proving it read
        // what the store wrote.
        expect_too_many_enabled(validate_ssv_builder_constraints(dir.path()), over_cap);
    }

    // ==================== Auth-data derivation vectors ====================
    //
    // These pin the Lighthouse-side derivation this module's zero-length check mirrors
    // (`builder_config` resolves omitted `auth_data` to the URL's UTF-8 bytes). If the
    // derivation ever changed, the check's notion of "resolves to zero length" could
    // silently diverge from what actually goes on the wire.

    /// Omitted `auth_data` resolves to exactly the URL's UTF-8 bytes (builder-specs #165
    /// default).
    #[tokio::test]
    async fn derivation_vector_omitted_auth_data() {
        // Arrange
        let (_dir, store) = store_with(vec![definition(true, TEST_URL, None)]);

        // Act
        let captured = captured_auth_data(&store).await;

        // Assert
        assert_eq!(
            captured,
            vec![TEST_URL.as_bytes().to_vec()],
            "omitted auth_data should resolve to the URL's exact UTF-8 bytes"
        );
    }

    /// URLs differing only by a trailing slash derive DISTINCT auth data: the default is
    /// the URL bytes exactly as configured, with no canonicalization. A signature over the
    /// wrong variant would fail the builder's byte-exact verification.
    #[tokio::test]
    async fn derivation_vector_trailing_slash_distinct() {
        // Arrange
        let url_without_slash = TEST_URL;
        let url_with_slash = format!("{TEST_URL}/");
        let (_dir, store) = store_with(vec![
            definition(true, url_without_slash, None),
            definition(true, &url_with_slash, None),
        ]);

        // Act
        let captured = captured_auth_data(&store).await;

        // Assert
        let mut expected = vec![
            url_without_slash.as_bytes().to_vec(),
            url_with_slash.as_bytes().to_vec(),
        ];
        expected.sort();
        assert_eq!(
            captured, expected,
            "both trailing-slash variants should be signed, each over its own exact bytes"
        );
    }

    /// Explicit `auth_data` is signed verbatim; the URL-bytes default only applies when the
    /// field is omitted.
    #[tokio::test]
    async fn derivation_vector_explicit_hex() {
        // Arrange
        let explicit_auth = vec![0x12, 0x34, 0xab, 0xcd];
        let (_dir, store) = store_with(vec![definition(
            true,
            TEST_URL,
            Some(explicit_auth.clone()),
        )]);

        // Act
        let captured = captured_auth_data(&store).await;

        // Assert
        assert_eq!(
            captured,
            vec![explicit_auth],
            "explicit auth_data should be signed verbatim, not replaced by the URL default"
        );
    }
}
