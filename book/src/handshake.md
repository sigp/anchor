# SSV NodeInfo Handshake Protocol Specification

This document specifies the **SSV NodeInfo Handshake Protocol**. The protocol is used by SSV-based nodes to exchange basic node metadata and validate each other's identity when establishing a connection over Libp2p under a dedicated protocol ID.

---

## Table of Contents

- [1. Introduction](#1-introduction)
- [2. Definitions](#2-definitions)
    - [2.1 Terminology](#21-terminology)
    - [2.2 Domain Separation](#22-domain-separation)
- [3. Protocol Constants](#3-protocol-constants)
- [4. Data Structures](#4-data-structures)
    - [4.1 Envelope](#41-envelope)
    - [4.2 NodeInfo](#42-nodeinfo)
    - [4.3 NodeMetadata](#43-nodemetadata)
- [5. Serialization and Signing](#5-serialization-and-signing)
    - [5.1 Envelope Fields](#51-envelope-fields)
    - [5.2 NodeInfo JSON Layout](#52-nodeinfo-json-layout)
    - [5.3 Signature Preparation](#53-signature-preparation)
- [6. Handshake Protocol Flows](#6-handshake-protocol-flows)
    - [6.1 Protocol ID](#61-protocol-id)
    - [6.2 Request Phase](#62-request-phase)
    - [6.3 Response Phase](#63-response-phase)
    - [6.4 Network Mismatch Checks](#64-network-mismatch-checks)
- [7. Security Considerations](#7-security-considerations)
- [8. Rationale and Notes](#8-rationale-and-notes)
- [9. Examples](#9-examples)

---

## 1. Introduction

The SSV NodeInfo Handshake Protocol defines how two SSV nodes exchange, sign, and verify each other's **NodeInfo**, which includes a `network_id` (such as "holesky", "prater", etc.) and optional metadata about node software versions or subnets. The protocol uses a request-response style handshake over Libp2p under a dedicated protocol ID.

The high-level handshake steps are:

1. **Requester** sends an Envelope (containing its NodeInfo) to the peer.
2. **Responder** verifies this Envelope, checks the `network_id`, and replies with its own Envelope.
3. **Requester** verifies the responder's Envelope.
4. Both sides proceed if verification succeeds; otherwise, the handshake is considered failed.

---

## 2. Definitions

### 2.1 Terminology

| **Term**     | **Definition**                                                                                                                                         |
|--------------|---------------------------------------------------------------------------------------------------------------------------------------------------------|
| **Envelope** | A Protobuf-encoded message containing a `public_key`, `payload_type`, `payload`, and `signature` (covering a domain-separated concatenation of fields). |
| **NodeInfo** | A JSON-based structure holding key node attributes like `network_id` plus optional metadata.                                                            |
| **Handshake**| The request-response exchange of Envelopes between two nodes at connection time.                                                                        |

### 2.2 Domain Separation

- **Domain**: `"ssv"`.  
  Used to separate signatures for different contexts or protocols.

---

## 3. Protocol Constants

| **Name**       | **Value**          | **Description**                                      |
|----------------|--------------------|------------------------------------------------------|
| `DOMAIN`       | `ssv`             | Fixed ASCII text used during signature generation.   |
| `PAYLOAD_TYPE` | `ssv/nodeinfo`    | Identifies the payload as an SSV NodeInfo structure. |
| `PROTOCOL_ID`  | `/ssv/info/0.0.1` | Libp2p protocol ID used for the handshake.           |

---

## 4. Data Structures

### 4.1 Envelope

The Envelope is a Protobuf message:

```protobuf
message Envelope {
  bytes public_key   = 1;
  bytes payload_type = 2;
  bytes payload      = 3;
  bytes signature    = 5;
}
```

### 4.2 NodeInfo

```
NodeInfo:
- network_id: String
- metadata: NodeMetadata (optional)
```

### 4.3 NodeMetadata

```
NodeMetadata:
- node_version: String
- execution_node: String
- consensus_node: String
- subnets: String
```

---

## 5. Serialization and Signing

### 5.1 Envelope Fields

1. **public_key**
    - Sender’s public key in serialized form (e.g., compressed Secp256k1 or raw Ed25519 bytes).
    - The public key is encoded and decoded using Protobuf.
    - For reference, Libp2p has a [Peer Ids and Keys](https://github.com/libp2p/specs/blob/master/peer-ids/peer-ids.md), which may be consulted for consistent handling across implementations.

2. **payload_type**
    - MUST be `"ssv/nodeinfo"` in this protocol.
    - Used to identify how to interpret `payload`.

3. **payload**
    - Contains `NodeInfo` data in JSON (described below).

4. **signature**
    - A cryptographic signature covering `DOMAIN || payload_type || payload`.

### 5.2 NodeInfo JSON Layout

Internally, the protocol uses a “legacy” layout for `NodeInfo` serialization, with a top-level JSON structure:

```json
{
  "Entries": [
    "",                       // (Index 0) Old forkVersion, not used
    "<network_id>",           // (Index 1) The NodeInfo.network_id
    "<json-encoded metadata>" // (Index 2) if NodeMetadata is present
  ]
}
```

- If the array has fewer than 2 entries, the payload is invalid.
- If the array has 3 entries, the 3rd entry is a JSON object for metadata, for example:

```json
{
  "NodeVersion": "...",
  "ExecutionNode": "...",
  "ConsensusNode": "...",
  "Subnets": "..."
}
```

### 5.3 Signature Preparation

To **sign** an Envelope, implementations:

1. Construct the unsigned message:
   ```
   unsigned_message = DOMAIN || payload_type || payload
   ```
2. Sign `unsigned_message` using the node’s private key.
3. Write the resulting signature to `signature`.

To **verify** an Envelope:

1. Recompute the `unsigned_message`.
2. Verify using `public_key` against `signature`.

If verification fails, the handshake **MUST** abort.

---

## 6. Handshake Protocol Flows

### 6.1 Protocol ID

Both peers must speak the protocol identified by:

```
/ssv/info/0.0.1
```

### 6.2 Request Phase

1. **Build Envelope**
    - The initiating node (Requester) serializes its `NodeInfo` into JSON (the `payload`).
    - Sets `payload_type = "ssv/nodeinfo"`.
    - Prepends `DOMAIN = "ssv"` when computing the signature.
    - Places the resulting `public_key` and `signature` into the Envelope.

2. **Send Request**
    - The requester sends this Envelope as the request.

3. **Wait for Response**
    - The requester awaits the single response from the Responder.

### 6.3 Response Phase

1. **Receive & Verify**
    - The responder verifies the incoming Envelope:
        - Check signature correctness.
        - Extract `NodeInfo`.
        - Validate `network_id` if necessary (see [6.4](#64-network-mismatch-checks)).

2. **Build Response**
    - If valid, the responder builds and signs its own Envelope containing its `NodeInfo`.

3. **Send Response**
    - The responder sends the Envelope back to the requester.

4. **Requester Verifies**
    - The requester verifies the signature, parses `NodeInfo`, and checks `network_id`.

### 6.4 Network Mismatch Checks

- Implementations **MUST** check whether the received `NodeInfo`’s `network_id` matches their local `network_id`.
- If they mismatch, the implementation **SHOULD** reject the connection.

---

## 7. Security Considerations

- **Signature Validation** is mandatory. Any failure to verify the Envelope’s signature indicates an invalid handshake.
- **Public Key Authenticity**: The Envelope’s `public_key` is not implicitly trusted. It must match the verified signature.
- **Network Mismatch**: Avoid bridging distinct SSV or Ethereum networks. Peers claiming the wrong `network_id` should be rejected.
- **Payload Size**: Although `NodeInfo` is generally small, implementations **SHOULD** impose a maximum bound for payload. Any request or response exceeding this size limit **SHOULD** be rejected.

---

## 8. Rationale and Notes

- Using a Protobuf-based Envelope simplifies cross-language interoperability.
- The domain separation string (`"ssv"`) prevents signature reuse in other contexts.
- The “legacy” `Entries` layout ensures backward-compatibility with older SSV implementations.

---

## 9. Examples

### 9.1 Example Envelope in Hex

An example Envelope could be hex-encoded as:

```
0a250802122102ba6a707dcec6c60ba2793d52123d34b22556964fc798d4aa88ffc41a00e42407120c7373762f6e6f6465696e666f1aa5017b22456e7472696573223a5b22222c22686f6c65736b79222c227b5c224e6f646556657273696f6e5c223a5c22676574682f785c222c5c22457865637574696f6e4e6f64655c223a5c22676574682f785c222c5c22436f6e73656e7375734e6f64655c223a5c22707279736d2f785c222c5c225375626e6574735c223a5c2230303030303030303030303030303030303030303030303030303030303030303030305c227d225d7d2a473045022100b8a2a668113330369e74b86ec818a87009e2a351f7ee4c0e431e1f659dd1bc3f02202b1ebf418efa7fb0541f77703bea8563234a1b70b8391d43daa40b6e7c3fcc84
```

Decoding reveals (high-level view):

```
Envelope {
  public_key   = <raw bytes>,
  payload_type = "ssv/nodeinfo",
  payload      = {
    "Entries": [
      "",
      "holesky",
      "{\"NodeVersion\":\"geth/x\",\"ExecutionNode\":\"geth/x\",\"ConsensusNode\":\"prysm/x\",\"Subnets\":\"00000000000000000000000000000000\"}"
    ]
  },
  signature    = <signature bytes>
}
```

### 9.2 Verifying the Envelope

1. Recompute: `domain = "ssv"`
   ```
   unsigned_message = "ssv" || "ssv/nodeinfo" || payload_bytes
   ```
2. Verify signature with `public_key`.
3. Parse payload JSON => parse `NodeInfo` => check `network_id`.
