//! SSV-specific validation of `builder_definitions.yml`, run once at startup after
//! Lighthouse's `BuilderStore` has loaded and validated the same file.
//!
//! Lighthouse validates against the beacon-API wire bounds, including per-validator
//! overrides, but it is SSV-unaware. Anchor adds the lower SSV entry cap and validates
//! disabled global entries that Lighthouse deliberately ignores:
//!
//! - SIP-94 §5 caps configured builder entries at 8 per validator. Excess entries load fine but
//!   their request-auth signing roots exceed the gossip root budget peers enforce, so which
//!   builders survive depends on per-peer message arrival order: nondeterministic, silent
//!   sub-quorum drops. Failing startup is the only loud signal.
//! - A zero-length auth `data` is invalid per SIP-94 §5 and rejected by go-ssv at config load.
//!   Lighthouse instead drops such a global builder during resolution. Rejecting it at startup
//!   keeps a config that go-ssv operators cannot even load from silently half-working on an Anchor
//!   operator.
//! - An entry's `builder_pubkeys` list is bounded at 64 (`MAX_BUILDER_PUBKEYS`) on the wire, but
//!   Lighthouse's global load validation never inspects it: an oversized list loads fine and
//!   `builder_config` then omits the builder during resolution, exactly the silent-drop failure
//!   mode above. Not an SSV constraint (go-ssv does not check it at load either), just fail-fast
//!   for a bound Lighthouse misses; redundant the moment Lighthouse bounds it at load, so revisit
//!   at the next pin bump.
//!
//! `BuilderStore`'s container type is crate-private, so the file is re-read here with a
//! minimal wrapper over the exported definition types and the same `yaml_serde` parser
//! Lighthouse uses. The round-trip tests write global and per-validator configuration
//! through the real store before exercising this wrapper.
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

use std::{collections::BTreeMap, fs::File, path::Path};

use builder_store::{BuilderDefinition, BuilderStore, ValidatorBuilderConfig};
use builder_types::MaxBuilderPubkeys;
use serde::Deserialize;
use typenum::Unsigned;

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
    #[serde(default)]
    validator_configs: BTreeMap<String, ValidatorBuilderConfig>,
}

/// A failure opening the builder definitions store or a violation of the SSV
/// builder-config constraints.
///
/// The SSV-specific variants carry only entry indices and counts, never URLs or auth
/// bytes: builder URLs may embed credentials, and auth `data` must stay out of logs
/// entirely. The `Store` display retains only a static error category because
/// Lighthouse store errors may contain a builder URL with embedded credentials.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Lighthouse's `BuilderStore` could not open, create, or validate the file.
    #[error("builder definitions store: {}", store_error_reason(.0))]
    Store(builder_store::Error),
    /// The definitions file could not be opened. `BuilderStore` creates it before this
    /// check runs, so this indicates a filesystem-level problem, not a missing file.
    #[error("unable to read the definitions file: {0}")]
    UnableToRead(std::io::Error),
    /// The definitions file could not be parsed. `BuilderStore` parsed the same bytes
    /// moments earlier, so this indicates either an external write racing startup or
    /// wrapper drift against a new Lighthouse pin.
    #[error("unable to parse the definitions file")]
    UnableToParse(yaml_serde::Error),
    /// More than [`MAX_SSV_BUILDER_ENTRIES`] enabled entries.
    #[error("{enabled} enabled builder entries exceed the SSV cap of {max} (SIP-94)")]
    TooManyEnabledEntries { enabled: usize, max: usize },
    /// The entry at `index` (file order, zero-based) resolves to zero-length auth
    /// `data` because it contains an explicit empty value (`auth_data: "0x"`). Omit
    /// `auth_data` to default to the lowercase ASCII hostname.
    #[error(
        "builder entry {index} resolves to zero-length auth data; omit `auth_data` to \
         default to the lowercase ASCII hostname"
    )]
    ZeroLengthAuthData { index: usize },
    /// The entry at `index` (file order, zero-based) configures more `builder_pubkeys`
    /// than the wire's `MAX_BUILDER_PUBKEYS`. Lighthouse accepts the global entry and
    /// would then omit the builder from block-production requests.
    #[error(
        "builder entry {index} configures {count} builder pubkeys, exceeding the wire cap \
         of {max}"
    )]
    TooManyBuilderPubkeys {
        index: usize,
        count: usize,
        max: usize,
    },
    /// A validator-specific override exceeds the SSV entry cap. An absent list inherits
    /// globals and an empty list explicitly disables direct builders, so only present
    /// non-empty lists are counted here.
    #[error(
        "validator {validator_pubkey} configures {count} builder entries, exceeding the SSV cap \
         of {max} (SIP-94)"
    )]
    TooManyValidatorEntries {
        validator_pubkey: String,
        count: usize,
        max: usize,
    },
}

/// Never format upstream payloads: even filesystem and parser errors may contain secrets.
fn store_error_reason(error: &builder_store::Error) -> &'static str {
    use builder_store::Error::*;
    match error {
        UnableToOpenFile(_) => "unable to open file",
        UnableToParseFile(_) => "unable to parse YAML",
        UnableToEncodeFile(_) => "unable to encode YAML",
        UnableToWriteFile(_) => "unable to write file",
        UnableToCreateValidatorDir(_) => "unable to create directory",
        DuplicateBuilderAuth(_) => "duplicate builder authentication",
        InvalidBuilderUrl(_) => "invalid builder URL",
        UnsupportedUrlScheme(_) => "unsupported builder URL scheme",
        TooManyEnabledBuilders { .. } => "too many enabled builders",
        TooManyBuilderPubkeys(_) => "too many builder public keys",
        EmptyAuthData(_) => "empty authentication data",
    }
}

/// Open (or create) `<dir>/builder_definitions.yml` through Lighthouse's `BuilderStore`,
/// then enforce the additional constraints on the same file, returning the store for
/// block-service wiring.
///
/// The single entry point keeps the ordering structural: Lighthouse's own load
/// validation always runs first (it also creates the file on first start), so this
/// pass never sees a file Lighthouse has not just vetted.
pub fn open_and_validate(builder_definitions_dir: &Path) -> Result<BuilderStore, Error> {
    let store = BuilderStore::open_or_create(builder_definitions_dir).map_err(Error::Store)?;
    validate_builder_constraints(builder_definitions_dir)?;
    Ok(store)
}

/// Enforce the builder-config constraints Lighthouse's load validation does not cover on
/// `<dir>/builder_definitions.yml`.
///
/// The per-entry checks cover ALL entries, disabled ones included: for auth `data` this
/// preserves Anchor's explicit-empty-auth policy, and the `builder_pubkeys` bound
/// follows the same shape so a disabled entry cannot become a deferred failure when
/// later enabled. Lighthouse validates default auth derivation for active URLs;
/// inactive URLs need no signing data. The entry cap counts ENABLED entries only:
/// disabled entries never reach the wire, and Lighthouse's own 64-entry wire cap also
/// counts enabled only.
fn validate_builder_constraints(builder_definitions_dir: &Path) -> Result<(), Error> {
    let path = builder_definitions_dir.join(BUILDER_DEFINITIONS_FILENAME);
    let file = File::open(&path).map_err(Error::UnableToRead)?;
    let config: BuilderDefinitionsFile =
        yaml_serde::from_reader(file).map_err(Error::UnableToParse)?;

    for (index, definition) in config.builders.iter().enumerate() {
        if definition
            .auth_data
            .as_ref()
            .is_some_and(|data| data.is_empty())
        {
            return Err(Error::ZeroLengthAuthData { index });
        }

        let count = definition.builder_pubkeys.len();
        if count > MaxBuilderPubkeys::USIZE {
            return Err(Error::TooManyBuilderPubkeys {
                index,
                count,
                max: MaxBuilderPubkeys::USIZE,
            });
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

    for (validator_pubkey, validator_config) in &config.validator_configs {
        let Some(builders) = &validator_config.builders else {
            continue;
        };
        let count = builders.len();
        if count > MAX_SSV_BUILDER_ENTRIES {
            return Err(Error::TooManyValidatorEntries {
                validator_pubkey: validator_pubkey.clone(),
                count,
                max: MAX_SSV_BUILDER_ENTRIES,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bls::{Keypair, PublicKeyBytes, Signature};
    use builder_store::ValidatorBuilderDefinition;
    use builder_types::{RequestAuth, RequestAuthData, SignedRequestAuth};
    use parking_lot::Mutex;
    use serde::Serialize;
    use tempfile::TempDir;
    use types::Slot;

    use super::*;

    /// The canonical fixture URL.
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

    fn validator_definition(url: &str, auth_data: Option<Vec<u8>>) -> ValidatorBuilderDefinition {
        ValidatorBuilderDefinition {
            url: url.parse().expect("fixture URL fits the ByteList limit"),
            auth_data: auth_data
                .map(|bytes| RequestAuthData::new(bytes).expect("auth data fits the limit")),
            builder_pubkeys: vec![],
            max_execution_payment: None,
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
    async fn captured_auth_data(
        store: &BuilderStore,
        validator_pubkey: &PublicKeyBytes,
    ) -> Vec<Vec<u8>> {
        let captured: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let capture = captured.clone();
        store
            .builder_config(validator_pubkey, move |data: RequestAuthData| {
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
        let result = validate_builder_constraints(dir.path());

        // Assert
        assert!(
            result.is_ok(),
            "{MAX_SSV_BUILDER_ENTRIES} enabled plus disabled entries should pass, got: \
             {result:?}"
        );
    }

    #[test]
    fn per_validator_cap_handles_inherit_disable_and_override() {
        let (dir, store) = store_with(
            (0..MAX_SSV_BUILDER_ENTRIES)
                .map(|index| definition(true, &indexed_url(index), None))
                .collect(),
        );
        let validator = Keypair::random().pk.compress();

        store
            .set_validator_config(
                &validator,
                ValidatorBuilderConfig {
                    builders: None,
                    ..Default::default()
                },
            )
            .expect("an omitted override list should inherit globals");
        assert_eq!(store.get_validator_config(&validator).builders.len(), 8);
        assert!(validate_builder_constraints(dir.path()).is_ok());

        store
            .set_validator_config(
                &validator,
                ValidatorBuilderConfig {
                    builders: Some(vec![]),
                    ..Default::default()
                },
            )
            .expect("an empty override list should disable direct builders");
        assert!(store.get_validator_config(&validator).builders.is_empty());
        assert!(validate_builder_constraints(dir.path()).is_ok());

        let overrides = (0..MAX_SSV_BUILDER_ENTRIES)
            .map(|index| validator_definition(&format!("https://override{index}.example"), None))
            .collect();
        store
            .set_validator_config(
                &validator,
                ValidatorBuilderConfig {
                    builders: Some(overrides),
                    ..Default::default()
                },
            )
            .expect("eight override entries fit Lighthouse and SSV bounds");
        assert_eq!(store.get_validator_config(&validator).builders.len(), 8);
        assert!(validate_builder_constraints(dir.path()).is_ok());

        let secret_url = "https://user:password@secret-builder.example/path";
        let secret_auth = b"do-not-log-auth-data".to_vec();
        let mut overrides: Vec<_> = (0..MAX_SSV_BUILDER_ENTRIES)
            .map(|index| validator_definition(&format!("https://override{index}.example"), None))
            .collect();
        overrides.push(validator_definition(secret_url, Some(secret_auth.clone())));
        store
            .set_validator_config(
                &validator,
                ValidatorBuilderConfig {
                    builders: Some(overrides),
                    ..Default::default()
                },
            )
            .expect("nine entries remain below Lighthouse's wire cap");

        let error = validate_builder_constraints(dir.path())
            .expect_err("nine validator-specific entries should exceed the SSV cap");
        match &error {
            Error::TooManyValidatorEntries {
                validator_pubkey,
                count,
                max,
            } => {
                assert_eq!(validator_pubkey, &validator.to_string());
                assert_eq!(*count, MAX_SSV_BUILDER_ENTRIES + 1);
                assert_eq!(*max, MAX_SSV_BUILDER_ENTRIES);
            }
            other => panic!("expected TooManyValidatorEntries, got: {other:?}"),
        }
        let rendered = error.to_string();
        assert!(!rendered.contains(secret_url));
        assert!(!rendered.contains("password"));
        assert!(!rendered.contains("do-not-log-auth-data"));
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
        match validate_builder_constraints(dir.path()) {
            Err(error @ Error::ZeroLengthAuthData { index }) => {
                assert_eq!(
                    index, 1,
                    "the error should carry the file-order entry index"
                );
                assert!(
                    error
                        .to_string()
                        .contains("default to the lowercase ASCII hostname")
                );
            }
            other => panic!("expected ZeroLengthAuthData, got: {other:?}"),
        }
    }

    /// Disabled entries never need hostname-derived signing data. This also explicitly
    /// permits an empty inactive URL, while explicit empty auth remains invalid above.
    #[test]
    fn disabled_urls_do_not_require_hostname_default_auth() {
        for url in ["https://user:password@", ""] {
            // Arrange: Lighthouse accepts inactive URLs without deriving auth data.
            let (dir, _) = store_with(vec![definition(false, url, None)]);

            // Act
            let result = open_and_validate(dir.path());

            // Assert
            assert!(
                result.is_ok(),
                "inactive URLs must not require default auth: {:?}",
                result.err()
            );
        }
    }

    /// Startup diagnostics must retain an actionable category without displaying the
    /// URL or opaque auth bytes carried by upstream errors.
    #[test]
    fn store_validation_errors_preserve_safe_categories() {
        // Arrange: bypass store insertion so startup itself validates each bad file.
        #[derive(Serialize)]
        struct FixtureFile {
            builders: Vec<BuilderDefinition>,
            validator_configs: BTreeMap<String, ValidatorBuilderConfig>,
        }
        let secret_url = "https://user:password@builder.example.com/path";
        let duplicate = definition(true, secret_url, Some(b"do-not-log-auth-data".to_vec()));
        let mut oversized =
            validator_definition(secret_url, Some(b"do-not-log-auth-data".to_vec()));
        oversized.builder_pubkeys = test_pubkeys(MaxBuilderPubkeys::USIZE + 1);
        let validator = Keypair::random().pk.compress().to_string();
        let cases = [
            (
                "duplicate builder",
                vec![duplicate.clone(), duplicate],
                None,
            ),
            (
                "invalid builder URL",
                vec![definition(true, "https://user:password@", None)],
                None,
            ),
            (
                "unsupported builder URL scheme",
                vec![definition(
                    true,
                    "ftp://user:password@builder.example.com",
                    None,
                )],
                None,
            ),
            (
                "empty authentication data",
                vec![],
                Some(validator_definition(secret_url, Some(vec![]))),
            ),
            ("too many builder public keys", vec![], Some(oversized)),
        ];
        for (category, builders, validator_definition) in cases {
            let dir = TempDir::new().expect("tempdir");
            let validator_configs = validator_definition
                .map(|definition| {
                    BTreeMap::from([(
                        validator.clone(),
                        ValidatorBuilderConfig {
                            builders: Some(vec![definition]),
                            ..Default::default()
                        },
                    )])
                })
                .unwrap_or_default();
            let fixture = FixtureFile {
                builders,
                validator_configs,
            };
            let file =
                File::create(dir.path().join(BUILDER_DEFINITIONS_FILENAME)).expect("fixture file");
            yaml_serde::to_writer(file, &fixture).expect("fixture should serialize");

            // Act
            let error = match open_and_validate(dir.path()) {
                Err(error) => error,
                Ok(_) => panic!("{category} should fail store validation"),
            };
            let rendered = error.to_string();

            // Assert
            assert!(matches!(error, Error::Store(_)));
            assert!(
                rendered.contains(category),
                "expected {category:?} in {rendered:?}"
            );
            for secret in [
                secret_url,
                "user",
                "password",
                "do-not-log-auth-data",
                "646f2d6e6f742d6c6f672d617574682d64617461",
            ] {
                assert!(
                    !rendered.contains(secret),
                    "startup error must omit sensitive values"
                );
            }
        }
    }

    #[test]
    fn store_file_open_error_preserves_safe_category() {
        // Arrange: inject the portable filesystem failure without OS permission assumptions.
        let error = Error::Store(builder_store::Error::UnableToOpenFile(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "do-not-log-auth-data",
        )));

        // Act
        let rendered = error.to_string();

        // Assert
        assert!(rendered.contains("unable to open"));
        assert!(!rendered.contains("do-not-log-auth-data"));
    }

    /// A concurrent file edit can make the second read fail parsing after the store has
    /// loaded. Parser diagnostics include invalid scalar values, which may be secrets.
    #[test]
    fn second_read_parse_error_does_not_render_sensitive_scalar() {
        // Arrange: obtain a real parser error carrying the invalid input value.
        let sensitive_marker = "sensitive_auth_material_do_not_log";
        let input = format!("builders: {sensitive_marker}\n");
        let parse_error = match yaml_serde::from_str::<BuilderDefinitionsFile>(&input) {
            Err(error) => error,
            Ok(_) => panic!("a scalar builders field must fail deserialization"),
        };
        assert!(
            parse_error.to_string().contains(sensitive_marker),
            "fixture must expose the sensitive scalar in the underlying parser error"
        );

        // Act: this is the same wrapper used by the startup second-read path.
        let rendered = Error::UnableToParse(parse_error).to_string();

        // Assert
        assert!(rendered.contains("unable to parse"));
        assert!(
            !rendered.contains(sensitive_marker),
            "startup Display must redact parser input values: {rendered}"
        );
    }

    // ==================== Builder-pubkeys bound tests ====================

    /// A pubkey list of the given length. The bound only counts entries, so identical
    /// keys are fine.
    fn test_pubkeys(count: usize) -> Vec<PublicKeyBytes> {
        vec![PublicKeyBytes::deserialize(&[1u8; 48]).expect("48 bytes is a valid pubkey"); count]
    }

    /// Exactly the wire cap of `builder_pubkeys` passes; one over fails at startup with
    /// the entry's file-order index and both counts. The over-cap entry is DISABLED to pin
    /// that the bound covers all entries (the enabled case follows a fortiori, since the
    /// per-entry loop does not filter). Without this check the entry loads fine and
    /// `builder_config` omits the builder, with only an error log, at every produce.
    #[test]
    fn builder_pubkeys_over_wire_cap_fail() {
        // Arrange: an enabled entry at exactly the wire cap.
        let mut at_cap = definition(true, TEST_URL, None);
        at_cap.builder_pubkeys = test_pubkeys(MaxBuilderPubkeys::USIZE);
        let (dir, store) = store_with(vec![at_cap]);

        // Act + Assert: the cap itself passes.
        let result = validate_builder_constraints(dir.path());
        assert!(
            result.is_ok(),
            "exactly {} builder pubkeys should pass, got: {result:?}",
            MaxBuilderPubkeys::USIZE
        );

        // Arrange: a DISABLED second entry one key over the cap. The insert succeeding is
        // itself part of the pin: Lighthouse's own load validation never inspects
        // `builder_pubkeys`, which is why this check exists.
        let mut over_cap = definition(false, "https://disabled.example.com", None);
        over_cap.builder_pubkeys = test_pubkeys(MaxBuilderPubkeys::USIZE + 1);
        store
            .insert(over_cap)
            .expect("Lighthouse load validation does not bound builder_pubkeys");

        // Act + Assert
        match validate_builder_constraints(dir.path()) {
            Err(Error::TooManyBuilderPubkeys { index, count, max }) => {
                assert_eq!(
                    index, 1,
                    "the error should carry the file-order entry index"
                );
                assert_eq!(count, MaxBuilderPubkeys::USIZE + 1);
                assert_eq!(max, MaxBuilderPubkeys::USIZE);
            }
            other => panic!("expected TooManyBuilderPubkeys, got: {other:?}"),
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
        let result = validate_builder_constraints(dir.path());
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
        expect_too_many_enabled(open_and_validate(dir.path()).map(|_| ()), over_cap);
    }

    // ==================== Auth-data derivation vectors ====================
    //
    // These pin the Lighthouse-side derivation used for enabled builders on the wire.

    /// Equivalent URL spellings resolve to the same lowercase ASCII hostname.
    #[tokio::test]
    async fn derivation_vector_omitted_auth_data() {
        let complex_url =
            "HTTPS://User:Password@Builder.Example.Com:443/bids?network=hoodi#fragment";
        let (_dir, store) = store_with(vec![
            definition(true, TEST_URL, None),
            definition(true, complex_url, None),
        ]);
        let validator = Keypair::random().pk.compress();

        let captured = captured_auth_data(&store, &validator).await;

        assert_eq!(
            captured,
            vec![
                b"builder.example.com".to_vec(),
                b"builder.example.com".to_vec(),
            ],
            "omitted auth_data should resolve to lowercase hostname bytes"
        );
    }

    /// Explicit `auth_data` is signed verbatim; hostname defaulting applies only when omitted.
    #[tokio::test]
    async fn derivation_vector_explicit_hex() {
        let explicit_auth = vec![0x12, 0x34, 0xab, 0xcd];
        let (dir, store) = store_with(vec![]);
        let validator = Keypair::random().pk.compress();
        store
            .set_validator_config(
                &validator,
                ValidatorBuilderConfig {
                    builders: Some(vec![validator_definition(
                        TEST_URL,
                        Some(explicit_auth.clone()),
                    )]),
                    ..Default::default()
                },
            )
            .expect("explicit per-validator auth should be accepted");
        assert!(validate_builder_constraints(dir.path()).is_ok());

        let captured = captured_auth_data(&store, &validator).await;

        assert_eq!(
            captured,
            vec![explicit_auth],
            "explicit auth_data should be signed verbatim"
        );
    }
}
