#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use ssv_types::{
    consensus::QbftMessageType, message::MsgType, msgid::MessageId,
    partial_sig::PartialSignatureKind,
};
use ssz::{Decode, Encode};

#[derive(Arbitrary, Debug)]
enum SszTarget {
    MessageId(MessageId),
    MsgType(MsgType),
    QbftMessageType(QbftMessageType),
    PartialSignatureKind(PartialSignatureKind),
}

fuzz_target!(|target: SszTarget| {
    match target {
        SszTarget::MessageId(message_id) => {
            let encoded = message_id.as_ssz_bytes();
            match MessageId::from_ssz_bytes(&encoded) {
                Ok(decoded) => assert_eq!(message_id, decoded),
                Err(e) => panic!("Failed to decode MessageId: {e:?}"),
            }
        }
        SszTarget::MsgType(msg_type) => {
            let encoded = msg_type.as_ssz_bytes();
            match MsgType::from_ssz_bytes(&encoded) {
                Ok(decoded) => assert_eq!(msg_type, decoded),
                Err(e) => panic!("Failed to decode MsgType: {e:?}"),
            }
        }
        SszTarget::QbftMessageType(qbft_msg_type) => {
            let encoded = qbft_msg_type.as_ssz_bytes();
            match QbftMessageType::from_ssz_bytes(&encoded) {
                Ok(decoded) => assert_eq!(qbft_msg_type, decoded),
                Err(e) => panic!("Failed to decode QbftMessageType: {e:?}"),
            }
        }
        SszTarget::PartialSignatureKind(partial_sig_kind) => {
            let encoded = partial_sig_kind.as_ssz_bytes();
            match PartialSignatureKind::from_ssz_bytes(&encoded) {
                Ok(decoded) => assert_eq!(partial_sig_kind, decoded),
                Err(e) => panic!("Failed to decode PartialSignatureKind: {e:?}"),
            }
        }
    }
});
