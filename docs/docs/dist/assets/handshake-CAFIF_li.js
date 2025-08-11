import{u as r,j as e}from"./index-CPZ5_ZSA.js";const l={title:"SSV NodeInfo Handshake Protocol Specification",description:"undefined"};function i(s){const n={a:"a",br:"br",code:"code",div:"div",h1:"h1",h2:"h2",h3:"h3",header:"header",hr:"hr",li:"li",ol:"ol",p:"p",pre:"pre",span:"span",strong:"strong",table:"table",tbody:"tbody",td:"td",th:"th",thead:"thead",tr:"tr",ul:"ul",...r(),...s.components};return e.jsxs(e.Fragment,{children:[e.jsx(n.header,{children:e.jsxs(n.h1,{id:"ssv-nodeinfo-handshake-protocol-specification",children:["SSV NodeInfo Handshake Protocol Specification",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#ssv-nodeinfo-handshake-protocol-specification",children:e.jsx(n.div,{"data-autolink-icon":!0})})]})}),`
`,e.jsxs(n.p,{children:["This document specifies the ",e.jsx(n.strong,{children:"SSV NodeInfo Handshake Protocol"}),". The protocol is used by SSV-based nodes to exchange basic node metadata and validate each other's identity when establishing a connection over Libp2p under a dedicated protocol ID."]}),`
`,e.jsx(n.hr,{}),`
`,e.jsxs(n.h2,{id:"table-of-contents",children:["Table of Contents",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#table-of-contents",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsx(n.li,{children:e.jsx(n.a,{href:"#1-introduction",children:"1. Introduction"})}),`
`,e.jsxs(n.li,{children:[e.jsx(n.a,{href:"#2-definitions",children:"2. Definitions"}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsx(n.li,{children:e.jsx(n.a,{href:"#21-terminology",children:"2.1 Terminology"})}),`
`,e.jsx(n.li,{children:e.jsx(n.a,{href:"#22-domain-separation",children:"2.2 Domain Separation"})}),`
`]}),`
`]}),`
`,e.jsx(n.li,{children:e.jsx(n.a,{href:"#3-protocol-constants",children:"3. Protocol Constants"})}),`
`,e.jsxs(n.li,{children:[e.jsx(n.a,{href:"#4-data-structures",children:"4. Data Structures"}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsx(n.li,{children:e.jsx(n.a,{href:"#41-envelope",children:"4.1 Envelope"})}),`
`,e.jsx(n.li,{children:e.jsx(n.a,{href:"#42-nodeinfo",children:"4.2 NodeInfo"})}),`
`,e.jsx(n.li,{children:e.jsx(n.a,{href:"#43-nodemetadata",children:"4.3 NodeMetadata"})}),`
`]}),`
`]}),`
`,e.jsxs(n.li,{children:[e.jsx(n.a,{href:"#5-serialization-and-signing",children:"5. Serialization and Signing"}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsx(n.li,{children:e.jsx(n.a,{href:"#51-envelope-fields",children:"5.1 Envelope Fields"})}),`
`,e.jsx(n.li,{children:e.jsx(n.a,{href:"#52-nodeinfo-json-layout",children:"5.2 NodeInfo JSON Layout"})}),`
`,e.jsx(n.li,{children:e.jsx(n.a,{href:"#53-signature-preparation",children:"5.3 Signature Preparation"})}),`
`]}),`
`]}),`
`,e.jsxs(n.li,{children:[e.jsx(n.a,{href:"#6-handshake-protocol-flows",children:"6. Handshake Protocol Flows"}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsx(n.li,{children:e.jsx(n.a,{href:"#61-protocol-id",children:"6.1 Protocol ID"})}),`
`,e.jsx(n.li,{children:e.jsx(n.a,{href:"#62-request-phase",children:"6.2 Request Phase"})}),`
`,e.jsx(n.li,{children:e.jsx(n.a,{href:"#63-response-phase",children:"6.3 Response Phase"})}),`
`,e.jsx(n.li,{children:e.jsx(n.a,{href:"#64-network-mismatch-checks",children:"6.4 Network Mismatch Checks"})}),`
`]}),`
`]}),`
`,e.jsx(n.li,{children:e.jsx(n.a,{href:"#7-security-considerations",children:"7. Security Considerations"})}),`
`,e.jsx(n.li,{children:e.jsx(n.a,{href:"#8-rationale-and-notes",children:"8. Rationale and Notes"})}),`
`,e.jsx(n.li,{children:e.jsx(n.a,{href:"#9-examples",children:"9. Examples"})}),`
`]}),`
`,e.jsx(n.hr,{}),`
`,e.jsxs(n.h2,{id:"1-introduction",children:["1. Introduction",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#1-introduction",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.p,{children:["The SSV NodeInfo Handshake Protocol defines how two SSV nodes exchange, sign, and verify each other's ",e.jsx(n.strong,{children:"NodeInfo"}),", which includes a ",e.jsx(n.code,{children:"network_id"}),' (such as "holesky", "prater", etc.) and optional metadata about node software versions or subnets. The protocol uses a request-response style handshake over Libp2p under a dedicated protocol ID.']}),`
`,e.jsx(n.p,{children:"The high-level handshake steps are:"}),`
`,e.jsxs(n.ol,{children:[`
`,e.jsxs(n.li,{children:[e.jsx(n.strong,{children:"Requester"})," sends an Envelope (containing its NodeInfo) to the peer."]}),`
`,e.jsxs(n.li,{children:[e.jsx(n.strong,{children:"Responder"})," verifies this Envelope, checks the ",e.jsx(n.code,{children:"network_id"}),", and replies with its own Envelope."]}),`
`,e.jsxs(n.li,{children:[e.jsx(n.strong,{children:"Requester"})," verifies the responder's Envelope."]}),`
`,e.jsx(n.li,{children:"Both sides proceed if verification succeeds; otherwise, the handshake is considered failed."}),`
`]}),`
`,e.jsx(n.hr,{}),`
`,e.jsxs(n.h2,{id:"2-definitions",children:["2. Definitions",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#2-definitions",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.h3,{id:"21-terminology",children:["2.1 Terminology",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#21-terminology",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.table,{children:[e.jsx(n.thead,{children:e.jsxs(n.tr,{children:[e.jsx(n.th,{children:e.jsx(n.strong,{children:"Term"})}),e.jsx(n.th,{children:e.jsx(n.strong,{children:"Definition"})})]})}),e.jsxs(n.tbody,{children:[e.jsxs(n.tr,{children:[e.jsx(n.td,{children:e.jsx(n.strong,{children:"Envelope"})}),e.jsxs(n.td,{children:["A Protobuf-encoded message containing a ",e.jsx(n.code,{children:"public_key"}),", ",e.jsx(n.code,{children:"payload_type"}),", ",e.jsx(n.code,{children:"payload"}),", and ",e.jsx(n.code,{children:"signature"})," (covering a domain-separated concatenation of fields)."]})]}),e.jsxs(n.tr,{children:[e.jsx(n.td,{children:e.jsx(n.strong,{children:"NodeInfo"})}),e.jsxs(n.td,{children:["A JSON-based structure holding key node attributes like ",e.jsx(n.code,{children:"network_id"})," plus optional metadata."]})]}),e.jsxs(n.tr,{children:[e.jsx(n.td,{children:e.jsx(n.strong,{children:"Handshake"})}),e.jsx(n.td,{children:"The request-response exchange of Envelopes between two nodes at connection time."})]})]})]}),`
`,e.jsxs(n.h3,{id:"22-domain-separation",children:["2.2 Domain Separation",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#22-domain-separation",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsxs(n.li,{children:[e.jsx(n.strong,{children:"Domain"}),": ",e.jsx(n.code,{children:'"ssv"'}),".",e.jsx(n.br,{}),`
`,"Used to separate signatures for different contexts or protocols."]}),`
`]}),`
`,e.jsx(n.hr,{}),`
`,e.jsxs(n.h2,{id:"3-protocol-constants",children:["3. Protocol Constants",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#3-protocol-constants",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.table,{children:[e.jsx(n.thead,{children:e.jsxs(n.tr,{children:[e.jsx(n.th,{children:e.jsx(n.strong,{children:"Name"})}),e.jsx(n.th,{children:e.jsx(n.strong,{children:"Value"})}),e.jsx(n.th,{children:e.jsx(n.strong,{children:"Description"})})]})}),e.jsxs(n.tbody,{children:[e.jsxs(n.tr,{children:[e.jsx(n.td,{children:e.jsx(n.code,{children:"DOMAIN"})}),e.jsx(n.td,{children:e.jsx(n.code,{children:"ssv"})}),e.jsx(n.td,{children:"Fixed ASCII text used during signature generation."})]}),e.jsxs(n.tr,{children:[e.jsx(n.td,{children:e.jsx(n.code,{children:"PAYLOAD_TYPE"})}),e.jsx(n.td,{children:e.jsx(n.code,{children:"ssv/nodeinfo"})}),e.jsx(n.td,{children:"Identifies the payload as an SSV NodeInfo structure."})]}),e.jsxs(n.tr,{children:[e.jsx(n.td,{children:e.jsx(n.code,{children:"PROTOCOL_ID"})}),e.jsx(n.td,{children:e.jsx(n.code,{children:"/ssv/info/0.0.1"})}),e.jsx(n.td,{children:"Libp2p protocol ID used for the handshake."})]})]})]}),`
`,e.jsx(n.hr,{}),`
`,e.jsxs(n.h2,{id:"4-data-structures",children:["4. Data Structures",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#4-data-structures",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.h3,{id:"41-envelope",children:["4.1 Envelope",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#41-envelope",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsx(n.p,{children:"The Envelope is a Protobuf message:"}),`
`,e.jsx(e.Fragment,{children:e.jsx(n.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0",children:e.jsxs(n.code,{children:[e.jsxs(n.span,{className:"line",children:[e.jsx(n.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"message"}),e.jsx(n.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:" Envelope"}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:" {"})]}),`
`,e.jsxs(n.span,{className:"line",children:[e.jsx(n.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"  bytes"}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:" public_key   "}),e.jsx(n.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"="}),e.jsx(n.span,{style:{color:"#005CC5","--shiki-dark":"#6CB6FF"},children:" 1"}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:";"})]}),`
`,e.jsxs(n.span,{className:"line",children:[e.jsx(n.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"  bytes"}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:" payload_type "}),e.jsx(n.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"="}),e.jsx(n.span,{style:{color:"#005CC5","--shiki-dark":"#6CB6FF"},children:" 2"}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:";"})]}),`
`,e.jsxs(n.span,{className:"line",children:[e.jsx(n.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"  bytes"}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:" payload      "}),e.jsx(n.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"="}),e.jsx(n.span,{style:{color:"#005CC5","--shiki-dark":"#6CB6FF"},children:" 3"}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:";"})]}),`
`,e.jsxs(n.span,{className:"line",children:[e.jsx(n.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"  bytes"}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:" signature    "}),e.jsx(n.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"="}),e.jsx(n.span,{style:{color:"#005CC5","--shiki-dark":"#6CB6FF"},children:" 5"}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:";"})]}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:"}"})})]})})}),`
`,e.jsxs(n.h3,{id:"42-nodeinfo",children:["4.2 NodeInfo",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#42-nodeinfo",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsx(e.Fragment,{children:e.jsx(n.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0",children:e.jsxs(n.code,{children:[e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"NodeInfo:"})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"- network_id: String"})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"- metadata: NodeMetadata (optional)"})})]})})}),`
`,e.jsxs(n.h3,{id:"43-nodemetadata",children:["4.3 NodeMetadata",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#43-nodemetadata",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsx(e.Fragment,{children:e.jsx(n.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0",children:e.jsxs(n.code,{children:[e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"NodeMetadata:"})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"- node_version: String"})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"- execution_node: String"})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"- consensus_node: String"})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"- subnets: String"})})]})})}),`
`,e.jsx(n.hr,{}),`
`,e.jsxs(n.h2,{id:"5-serialization-and-signing",children:["5. Serialization and Signing",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#5-serialization-and-signing",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.h3,{id:"51-envelope-fields",children:["5.1 Envelope Fields",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#51-envelope-fields",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.ol,{children:[`
`,e.jsxs(n.li,{children:[`
`,e.jsx(n.strong,{children:"public_key"}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsx(n.li,{children:"Sender’s public key in serialized form (e.g., compressed Secp256k1 or raw Ed25519 bytes)."}),`
`,e.jsx(n.li,{children:"The public key is encoded and decoded using Protobuf."}),`
`,e.jsxs(n.li,{children:["For reference, Libp2p has a ",e.jsx(n.a,{href:"https://github.com/libp2p/specs/blob/master/peer-ids/peer-ids.md",children:"Peer Ids and Keys"}),", which may be consulted for consistent handling across implementations."]}),`
`]}),`
`]}),`
`,e.jsxs(n.li,{children:[`
`,e.jsx(n.strong,{children:"payload_type"}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsxs(n.li,{children:["MUST be ",e.jsx(n.code,{children:'"ssv/nodeinfo"'})," in this protocol."]}),`
`,e.jsxs(n.li,{children:["Used to identify how to interpret ",e.jsx(n.code,{children:"payload"}),"."]}),`
`]}),`
`]}),`
`,e.jsxs(n.li,{children:[`
`,e.jsx(n.strong,{children:"payload"}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsxs(n.li,{children:["Contains ",e.jsx(n.code,{children:"NodeInfo"})," data in JSON (described below)."]}),`
`]}),`
`]}),`
`,e.jsxs(n.li,{children:[`
`,e.jsx(n.strong,{children:"signature"}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsxs(n.li,{children:["A cryptographic signature covering ",e.jsx(n.code,{children:"DOMAIN || payload_type || payload"}),"."]}),`
`]}),`
`]}),`
`]}),`
`,e.jsxs(n.h3,{id:"52-nodeinfo-json-layout",children:["5.2 NodeInfo JSON Layout",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#52-nodeinfo-json-layout",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.p,{children:["Internally, the protocol uses a “legacy” layout for ",e.jsx(n.code,{children:"NodeInfo"})," serialization, with a top-level JSON structure:"]}),`
`,e.jsx(e.Fragment,{children:e.jsx(n.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0",children:e.jsxs(n.code,{children:[e.jsx(n.span,{className:"line",children:e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:"{"})}),`
`,e.jsxs(n.span,{className:"line",children:[e.jsx(n.span,{style:{color:"#005CC5","--shiki-dark":"#8DDB8C"},children:'  "Entries"'}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:": ["})]}),`
`,e.jsxs(n.span,{className:"line",children:[e.jsx(n.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:'    ""'}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:",                       "}),e.jsx(n.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"// (Index 0) Old forkVersion, not used"})]}),`
`,e.jsxs(n.span,{className:"line",children:[e.jsx(n.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:'    "<network_id>"'}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:",           "}),e.jsx(n.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"// (Index 1) The NodeInfo.network_id"})]}),`
`,e.jsxs(n.span,{className:"line",children:[e.jsx(n.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:'    "<json-encoded metadata>"'}),e.jsx(n.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:" // (Index 2) if NodeMetadata is present"})]}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:"  ]"})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:"}"})})]})})}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsx(n.li,{children:"If the array has fewer than 2 entries, the payload is invalid."}),`
`,e.jsx(n.li,{children:"If the array has 3 entries, the 3rd entry is a JSON object for metadata, for example:"}),`
`]}),`
`,e.jsx(e.Fragment,{children:e.jsx(n.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0",children:e.jsxs(n.code,{children:[e.jsx(n.span,{className:"line",children:e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:"{"})}),`
`,e.jsxs(n.span,{className:"line",children:[e.jsx(n.span,{style:{color:"#005CC5","--shiki-dark":"#8DDB8C"},children:'  "NodeVersion"'}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:": "}),e.jsx(n.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:'"..."'}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:","})]}),`
`,e.jsxs(n.span,{className:"line",children:[e.jsx(n.span,{style:{color:"#005CC5","--shiki-dark":"#8DDB8C"},children:'  "ExecutionNode"'}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:": "}),e.jsx(n.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:'"..."'}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:","})]}),`
`,e.jsxs(n.span,{className:"line",children:[e.jsx(n.span,{style:{color:"#005CC5","--shiki-dark":"#8DDB8C"},children:'  "ConsensusNode"'}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:": "}),e.jsx(n.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:'"..."'}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:","})]}),`
`,e.jsxs(n.span,{className:"line",children:[e.jsx(n.span,{style:{color:"#005CC5","--shiki-dark":"#8DDB8C"},children:'  "Subnets"'}),e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:": "}),e.jsx(n.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:'"..."'})]}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:"}"})})]})})}),`
`,e.jsxs(n.h3,{id:"53-signature-preparation",children:["5.3 Signature Preparation",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#53-signature-preparation",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.p,{children:["To ",e.jsx(n.strong,{children:"sign"})," an Envelope, implementations:"]}),`
`,e.jsxs(n.ol,{children:[`
`,e.jsxs(n.li,{children:[`
`,e.jsx(n.p,{children:"Construct the unsigned message:"}),`
`,e.jsx(e.Fragment,{children:e.jsx(n.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0",children:e.jsx(n.code,{children:e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"unsigned_message = DOMAIN || payload_type || payload"})})})})}),`
`]}),`
`,e.jsxs(n.li,{children:[`
`,e.jsxs(n.p,{children:["Sign ",e.jsx(n.code,{children:"unsigned_message"})," using the node’s private key."]}),`
`]}),`
`,e.jsxs(n.li,{children:[`
`,e.jsxs(n.p,{children:["Write the resulting signature to ",e.jsx(n.code,{children:"signature"}),"."]}),`
`]}),`
`]}),`
`,e.jsxs(n.p,{children:["To ",e.jsx(n.strong,{children:"verify"})," an Envelope:"]}),`
`,e.jsxs(n.ol,{children:[`
`,e.jsxs(n.li,{children:["Recompute the ",e.jsx(n.code,{children:"unsigned_message"}),"."]}),`
`,e.jsxs(n.li,{children:["Verify using ",e.jsx(n.code,{children:"public_key"})," against ",e.jsx(n.code,{children:"signature"}),"."]}),`
`]}),`
`,e.jsxs(n.p,{children:["If verification fails, the handshake ",e.jsx(n.strong,{children:"MUST"})," abort."]}),`
`,e.jsx(n.hr,{}),`
`,e.jsxs(n.h2,{id:"6-handshake-protocol-flows",children:["6. Handshake Protocol Flows",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#6-handshake-protocol-flows",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.h3,{id:"61-protocol-id",children:["6.1 Protocol ID",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#61-protocol-id",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsx(n.p,{children:"Both peers must speak the protocol identified by:"}),`
`,e.jsx(e.Fragment,{children:e.jsx(n.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0",children:e.jsx(n.code,{children:e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"/ssv/info/0.0.1"})})})})}),`
`,e.jsxs(n.h3,{id:"62-request-phase",children:["6.2 Request Phase",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#62-request-phase",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.ol,{children:[`
`,e.jsxs(n.li,{children:[`
`,e.jsx(n.strong,{children:"Build Envelope"}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsxs(n.li,{children:["The initiating node (Requester) serializes its ",e.jsx(n.code,{children:"NodeInfo"})," into JSON (the ",e.jsx(n.code,{children:"payload"}),")."]}),`
`,e.jsxs(n.li,{children:["Sets ",e.jsx(n.code,{children:'payload_type = "ssv/nodeinfo"'}),"."]}),`
`,e.jsxs(n.li,{children:["Prepends ",e.jsx(n.code,{children:'DOMAIN = "ssv"'})," when computing the signature."]}),`
`,e.jsxs(n.li,{children:["Places the resulting ",e.jsx(n.code,{children:"public_key"})," and ",e.jsx(n.code,{children:"signature"})," into the Envelope."]}),`
`]}),`
`]}),`
`,e.jsxs(n.li,{children:[`
`,e.jsx(n.strong,{children:"Send Request"}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsx(n.li,{children:"The requester sends this Envelope as the request."}),`
`]}),`
`]}),`
`,e.jsxs(n.li,{children:[`
`,e.jsx(n.strong,{children:"Wait for Response"}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsx(n.li,{children:"The requester awaits the single response from the Responder."}),`
`]}),`
`]}),`
`]}),`
`,e.jsxs(n.h3,{id:"63-response-phase",children:["6.3 Response Phase",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#63-response-phase",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.ol,{children:[`
`,e.jsxs(n.li,{children:[`
`,e.jsx(n.strong,{children:"Receive & Verify"}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsxs(n.li,{children:["The responder verifies the incoming Envelope:",`
`,e.jsxs(n.ul,{children:[`
`,e.jsx(n.li,{children:"Check signature correctness."}),`
`,e.jsxs(n.li,{children:["Extract ",e.jsx(n.code,{children:"NodeInfo"}),"."]}),`
`,e.jsxs(n.li,{children:["Validate ",e.jsx(n.code,{children:"network_id"})," if necessary (see ",e.jsx(n.a,{href:"#64-network-mismatch-checks",children:"6.4"}),")."]}),`
`]}),`
`]}),`
`]}),`
`]}),`
`,e.jsxs(n.li,{children:[`
`,e.jsx(n.strong,{children:"Build Response"}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsxs(n.li,{children:["If valid, the responder builds and signs its own Envelope containing its ",e.jsx(n.code,{children:"NodeInfo"}),"."]}),`
`]}),`
`]}),`
`,e.jsxs(n.li,{children:[`
`,e.jsx(n.strong,{children:"Send Response"}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsx(n.li,{children:"The responder sends the Envelope back to the requester."}),`
`]}),`
`]}),`
`,e.jsxs(n.li,{children:[`
`,e.jsx(n.strong,{children:"Requester Verifies"}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsxs(n.li,{children:["The requester verifies the signature, parses ",e.jsx(n.code,{children:"NodeInfo"}),", and checks ",e.jsx(n.code,{children:"network_id"}),"."]}),`
`]}),`
`]}),`
`]}),`
`,e.jsxs(n.h3,{id:"64-network-mismatch-checks",children:["6.4 Network Mismatch Checks",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#64-network-mismatch-checks",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsxs(n.li,{children:["Implementations ",e.jsx(n.strong,{children:"MUST"})," check whether the received ",e.jsx(n.code,{children:"NodeInfo"}),"’s ",e.jsx(n.code,{children:"network_id"})," matches their local ",e.jsx(n.code,{children:"network_id"}),"."]}),`
`,e.jsxs(n.li,{children:["If they mismatch, the implementation ",e.jsx(n.strong,{children:"SHOULD"})," reject the connection."]}),`
`]}),`
`,e.jsx(n.hr,{}),`
`,e.jsxs(n.h2,{id:"7-security-considerations",children:["7. Security Considerations",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#7-security-considerations",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsxs(n.li,{children:[e.jsx(n.strong,{children:"Signature Validation"})," is mandatory. Any failure to verify the Envelope’s signature indicates an invalid handshake."]}),`
`,e.jsxs(n.li,{children:[e.jsx(n.strong,{children:"Public Key Authenticity"}),": The Envelope’s ",e.jsx(n.code,{children:"public_key"})," is not implicitly trusted. It must match the verified signature."]}),`
`,e.jsxs(n.li,{children:[e.jsx(n.strong,{children:"Network Mismatch"}),": Avoid bridging distinct SSV or Ethereum networks. Peers claiming the wrong ",e.jsx(n.code,{children:"network_id"})," should be rejected."]}),`
`,e.jsxs(n.li,{children:[e.jsx(n.strong,{children:"Payload Size"}),": Although ",e.jsx(n.code,{children:"NodeInfo"})," is generally small, implementations ",e.jsx(n.strong,{children:"SHOULD"})," impose a maximum bound for payload. Any request or response exceeding this size limit ",e.jsx(n.strong,{children:"SHOULD"})," be rejected."]}),`
`]}),`
`,e.jsx(n.hr,{}),`
`,e.jsxs(n.h2,{id:"8-rationale-and-notes",children:["8. Rationale and Notes",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#8-rationale-and-notes",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsx(n.li,{children:"Using a Protobuf-based Envelope simplifies cross-language interoperability."}),`
`,e.jsxs(n.li,{children:["The domain separation string (",e.jsx(n.code,{children:'"ssv"'}),") prevents signature reuse in other contexts."]}),`
`,e.jsxs(n.li,{children:["The “legacy” ",e.jsx(n.code,{children:"Entries"})," layout ensures backward-compatibility with older SSV implementations."]}),`
`]}),`
`,e.jsx(n.hr,{}),`
`,e.jsxs(n.h2,{id:"9-examples",children:["9. Examples",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#9-examples",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.h3,{id:"91-example-envelope-in-hex",children:["9.1 Example Envelope in Hex",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#91-example-envelope-in-hex",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsx(n.p,{children:"An example Envelope could be hex-encoded as:"}),`
`,e.jsx(e.Fragment,{children:e.jsx(n.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0",children:e.jsx(n.code,{children:e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"0a250802122102ba6a707dcec6c60ba2793d52123d34b22556964fc798d4aa88ffc41a00e42407120c7373762f6e6f6465696e666f1aa5017b22456e7472696573223a5b22222c22686f6c65736b79222c227b5c224e6f646556657273696f6e5c223a5c22676574682f785c222c5c22457865637574696f6e4e6f64655c223a5c22676574682f785c222c5c22436f6e73656e7375734e6f64655c223a5c22707279736d2f785c222c5c225375626e6574735c223a5c2230303030303030303030303030303030303030303030303030303030303030303030305c227d225d7d2a473045022100b8a2a668113330369e74b86ec818a87009e2a351f7ee4c0e431e1f659dd1bc3f02202b1ebf418efa7fb0541f77703bea8563234a1b70b8391d43daa40b6e7c3fcc84"})})})})}),`
`,e.jsx(n.p,{children:"Decoding reveals (high-level view):"}),`
`,e.jsx(e.Fragment,{children:e.jsx(n.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0",children:e.jsxs(n.code,{children:[e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"Envelope {"})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"  public_key   = <raw bytes>,"})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:'  payload_type = "ssv/nodeinfo",'})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"  payload      = {"})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:'    "Entries": ['})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:'      "",'})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:'      "holesky",'})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:'      "{\\"NodeVersion\\":\\"geth/x\\",\\"ExecutionNode\\":\\"geth/x\\",\\"ConsensusNode\\":\\"prysm/x\\",\\"Subnets\\":\\"00000000000000000000000000000000\\"}"'})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"    ]"})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"  },"})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"  signature    = <signature bytes>"})}),`
`,e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:"}"})})]})})}),`
`,e.jsxs(n.h3,{id:"92-verifying-the-envelope",children:["9.2 Verifying the Envelope",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#92-verifying-the-envelope",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.ol,{children:[`
`,e.jsxs(n.li,{children:[`
`,e.jsxs(n.p,{children:["Recompute: ",e.jsx(n.code,{children:'domain = "ssv"'})]}),`
`,e.jsx(e.Fragment,{children:e.jsx(n.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0",children:e.jsx(n.code,{children:e.jsx(n.span,{className:"line",children:e.jsx(n.span,{children:'unsigned_message = "ssv" || "ssv/nodeinfo" || payload_bytes'})})})})}),`
`]}),`
`,e.jsxs(n.li,{children:[`
`,e.jsxs(n.p,{children:["Verify signature with ",e.jsx(n.code,{children:"public_key"}),"."]}),`
`]}),`
`,e.jsxs(n.li,{children:[`
`,e.jsxs(n.p,{children:["Parse payload JSON => parse ",e.jsx(n.code,{children:"NodeInfo"})," => check ",e.jsx(n.code,{children:"network_id"}),"."]}),`
`]}),`
`]})]})}function a(s={}){const{wrapper:n}={...r(),...s.components};return n?e.jsx(n,{...s,children:e.jsx(i,{...s})}):i(s)}export{a as default,l as frontmatter};
