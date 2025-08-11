import{u as r,j as s}from"./index-CPZ5_ZSA.js";const a={title:"Contributing to Anchor",description:"undefined"};function i(n){const e={a:"a",code:"code",div:"div",h1:"h1",h2:"h2",h3:"h3",header:"header",li:"li",ol:"ol",p:"p",pre:"pre",span:"span",strong:"strong",ul:"ul",...r(),...n.components};return s.jsxs(s.Fragment,{children:[s.jsx(e.header,{children:s.jsxs(e.h1,{id:"contributing-to-anchor",children:["Contributing to Anchor",s.jsx(e.a,{"aria-hidden":"true",tabIndex:"-1",href:"#contributing-to-anchor",children:s.jsx(e.div,{"data-autolink-icon":!0})})]})}),`
`,s.jsx(e.p,{children:"Anchor welcomes contributions. If you are interested in contributing to to this project, and you want to learn Rust, feel free to join us building this project."}),`
`,s.jsx(e.p,{children:"To start contributing,"}),`
`,s.jsxs(e.ol,{children:[`
`,s.jsxs(e.li,{children:["Read our ",s.jsx(e.a,{href:"https://github.com/sigp/anchor/blob/stable/CONTRIBUTING.md",children:"how to contribute"})," document."]}),`
`,s.jsxs(e.li,{children:["Setup a ",s.jsx(e.a,{href:"/development_environment",children:"development environment"}),"."]}),`
`,s.jsxs(e.li,{children:["Browse through the ",s.jsx(e.a,{href:"https://github.com/sigp/anchor/issues",children:"open issues"}),`
(tip: look for the `,s.jsx(e.a,{href:"https://github.com/sigp/anchor/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22",children:`good first
issue`}),`
tag).`]}),`
`,s.jsx(e.li,{children:"Comment on an issue before starting work."}),`
`,s.jsx(e.li,{children:"Share your work via a pull-request."}),`
`]}),`
`,s.jsxs(e.h2,{id:"branches",children:["Branches",s.jsx(e.a,{"aria-hidden":"true",tabIndex:"-1",href:"#branches",children:s.jsx(e.div,{"data-autolink-icon":!0})})]}),`
`,s.jsx(e.p,{children:"Anchor maintains two permanent branches:"}),`
`,s.jsxs(e.ul,{children:[`
`,s.jsxs(e.li,{children:[s.jsx(e.a,{href:"https://github.com/sigp/anchor/tree/stable",children:s.jsx(e.code,{children:"stable"})}),": Always points to the latest stable release.",`
`,s.jsxs(e.ul,{children:[`
`,s.jsx(e.li,{children:"This is ideal for most users."}),`
`]}),`
`]}),`
`,s.jsxs(e.li,{children:[s.jsx(e.a,{href:"https://github.com/sigp/anchor/tree/unstable",children:s.jsx(e.code,{children:"unstable"})}),": Used for development, contains the latest PRs.",`
`,s.jsxs(e.ul,{children:[`
`,s.jsx(e.li,{children:"Developers should base their PRs on this branch."}),`
`]}),`
`]}),`
`]}),`
`,s.jsxs(e.h2,{id:"rust",children:["Rust",s.jsx(e.a,{"aria-hidden":"true",tabIndex:"-1",href:"#rust",children:s.jsx(e.div,{"data-autolink-icon":!0})})]}),`
`,s.jsxs(e.p,{children:["We adhere to Rust code conventions as outlined in the ",s.jsx(e.a,{href:"https://doc.rust-lang.org/nightly/style-guide/",children:s.jsx(e.strong,{children:`Rust
Styleguide`})}),"."]}),`
`,s.jsxs(e.p,{children:["Please use ",s.jsx(e.a,{href:"https://github.com/rust-lang/rust-clippy",children:"clippy"}),` and
`,s.jsx(e.a,{href:"https://github.com/rust-lang/rustfmt",children:"rustfmt"}),` to detect common mistakes and
inconsistent code formatting:`]}),`
`,s.jsx(s.Fragment,{children:s.jsx(e.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0",children:s.jsxs(e.code,{children:[s.jsxs(e.span,{className:"line",children:[s.jsx(e.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:"cargo"}),s.jsx(e.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:" clippy"}),s.jsx(e.span,{style:{color:"#005CC5","--shiki-dark":"#6CB6FF"},children:" --all"})]}),`
`,s.jsxs(e.span,{className:"line",children:[s.jsx(e.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:"cargo"}),s.jsx(e.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:" fmt"}),s.jsx(e.span,{style:{color:"#005CC5","--shiki-dark":"#6CB6FF"},children:" --all"}),s.jsx(e.span,{style:{color:"#005CC5","--shiki-dark":"#6CB6FF"},children:" --check"})]})]})})}),`
`,s.jsxs(e.h3,{id:"panics",children:["Panics",s.jsx(e.a,{"aria-hidden":"true",tabIndex:"-1",href:"#panics",children:s.jsx(e.div,{"data-autolink-icon":!0})})]}),`
`,s.jsxs(e.p,{children:["Generally, ",s.jsx(e.strong,{children:"panics should be avoided at all costs"}),`. Anchor operates in an
adversarial environment (the Internet) and it's a severe vulnerability if
people on the Internet can cause Anchor to crash via a panic.`]}),`
`,s.jsxs(e.p,{children:["Always prefer returning a ",s.jsx(e.code,{children:"Result"})," or ",s.jsx(e.code,{children:"Option"}),` over causing a panic. For
example, prefer `,s.jsx(e.code,{children:"array.get(1)?"})," over ",s.jsx(e.code,{children:"array[1]"}),"."]}),`
`,s.jsxs(e.p,{children:[`If you know there won't be a panic but can't express that to the compiler,
use `,s.jsx(e.code,{children:'.expect("Helpful message")'})," instead of ",s.jsx(e.code,{children:".unwrap()"}),`. Always provide
detailed reasoning in a nearby comment when making assumptions about panics.`]}),`
`,s.jsxs(e.h3,{id:"todos",children:["TODOs",s.jsx(e.a,{"aria-hidden":"true",tabIndex:"-1",href:"#todos",children:s.jsx(e.div,{"data-autolink-icon":!0})})]}),`
`,s.jsxs(e.p,{children:["All ",s.jsx(e.code,{children:"TODO"})," statements should be accompanied by a GitHub issue."]}),`
`,s.jsx(s.Fragment,{children:s.jsx(e.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0",children:s.jsxs(e.code,{children:[s.jsxs(e.span,{className:"line",children:[s.jsx(e.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"pub"}),s.jsx(e.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:" fn"}),s.jsx(e.span,{style:{color:"#6F42C1","--shiki-dark":"#DCBDFB"},children:" my_function"}),s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:"("}),s.jsx(e.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"&mut"}),s.jsx(e.span,{style:{color:"#005CC5","--shiki-dark":"#6CB6FF"},children:" self"}),s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:", _something "}),s.jsx(e.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"&"}),s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:"["}),s.jsx(e.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:"u8"}),s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:"]) "}),s.jsx(e.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"->"}),s.jsx(e.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:" Result"}),s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:"<"}),s.jsx(e.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:"String"}),s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:", "}),s.jsx(e.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:"Error"}),s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:"> {"})]}),`
`,s.jsx(e.span,{className:"line",children:s.jsx(e.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"  // TODO: something_here"})}),`
`,s.jsx(e.span,{className:"line",children:s.jsx(e.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"  // https://github.com/sigp/anchor/issues/XX"})}),`
`,s.jsx(e.span,{className:"line",children:s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:"}"})})]})})}),`
`,s.jsxs(e.h3,{id:"comments",children:["Comments",s.jsx(e.a,{"aria-hidden":"true",tabIndex:"-1",href:"#comments",children:s.jsx(e.div,{"data-autolink-icon":!0})})]}),`
`,s.jsx(e.strong,{children:"General Comments"}),`
`,s.jsxs(e.ul,{children:[`
`,s.jsxs(e.li,{children:["Prefer line (",s.jsx(e.code,{children:"//"}),") comments to block comments (",s.jsx(e.code,{children:"/* ... */"}),")"]}),`
`,s.jsx(e.li,{children:"Comments can appear on the line prior to the item or after a trailing space."}),`
`]}),`
`,s.jsx(s.Fragment,{children:s.jsx(e.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0",children:s.jsxs(e.code,{children:[s.jsx(e.span,{className:"line",children:s.jsx(e.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"// Comment for this struct"})}),`
`,s.jsxs(e.span,{className:"line",children:[s.jsx(e.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"struct"}),s.jsx(e.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:" Anchor"}),s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:" {}"})]}),`
`,s.jsxs(e.span,{className:"line",children:[s.jsx(e.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"fn"}),s.jsx(e.span,{style:{color:"#6F42C1","--shiki-dark":"#DCBDFB"},children:" validate_attestation"}),s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:"() {} "}),s.jsx(e.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"// A comment on the same line after a space"})]})]})})}),`
`,s.jsx(e.strong,{children:"Doc Comments"}),`
`,s.jsxs(e.ul,{children:[`
`,s.jsxs(e.li,{children:["The ",s.jsx(e.code,{children:"///"})," is used to generate comments for Docs."]}),`
`,s.jsx(e.li,{children:"The comments should come before attributes."}),`
`]}),`
`,s.jsx(s.Fragment,{children:s.jsx(e.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0",children:s.jsxs(e.code,{children:[s.jsx(e.span,{className:"line",children:s.jsx(e.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"/// Stores the core configuration for this instance."})}),`
`,s.jsx(e.span,{className:"line",children:s.jsx(e.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"/// This struct is general, other components may implement more"})}),`
`,s.jsx(e.span,{className:"line",children:s.jsx(e.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"/// specialized config structs."})}),`
`,s.jsxs(e.span,{className:"line",children:[s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:"#[derive("}),s.jsx(e.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:"Clone"}),s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:")]"})]}),`
`,s.jsxs(e.span,{className:"line",children:[s.jsx(e.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"pub"}),s.jsx(e.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:" struct"}),s.jsx(e.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:" Config"}),s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:" {"})]}),`
`,s.jsxs(e.span,{className:"line",children:[s.jsx(e.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"    pub"}),s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:" data_dir"}),s.jsx(e.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:":"}),s.jsx(e.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:" PathBuf"}),s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:","})]}),`
`,s.jsxs(e.span,{className:"line",children:[s.jsx(e.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"    pub"}),s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:" p2p_listen_port"}),s.jsx(e.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:":"}),s.jsx(e.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:" u16"}),s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:","})]}),`
`,s.jsx(e.span,{className:"line",children:s.jsx(e.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:"}"})})]})})}),`
`,s.jsxs(e.h3,{id:"rust-resources",children:["Rust Resources",s.jsx(e.a,{"aria-hidden":"true",tabIndex:"-1",href:"#rust-resources",children:s.jsx(e.div,{"data-autolink-icon":!0})})]}),`
`,s.jsxs(e.p,{children:[`Rust is an extremely powerful, low-level programming language that provides
freedom and performance to create powerful projects. The `,s.jsx(e.a,{href:"https://doc.rust-lang.org/stable/book/",children:`Rust
Book`}),` provides insight into the Rust
language and some of the coding style to follow (As well as acting as a great
introduction and tutorial for the language).`]}),`
`,s.jsx(e.p,{children:`Rust has a steep learning curve, but there are many resources to help. We
suggest:`}),`
`,s.jsxs(e.ul,{children:[`
`,s.jsx(e.li,{children:s.jsx(e.a,{href:"https://doc.rust-lang.org/stable/book/",children:"Rust Book"})}),`
`,s.jsx(e.li,{children:s.jsx(e.a,{href:"https://doc.rust-lang.org/stable/rust-by-example/",children:"Rust by example"})}),`
`,s.jsx(e.li,{children:s.jsx(e.a,{href:"http://cglab.ca/~abeinges/blah/too-many-lists/book/",children:"Learning Rust With Entirely Too Many Linked Lists"})}),`
`,s.jsx(e.li,{children:s.jsx(e.a,{href:"https://github.com/rustlings/rustlings",children:"Rustlings"})}),`
`,s.jsx(e.li,{children:s.jsx(e.a,{href:"https://exercism.io/tracks/rust",children:"Rust Exercism"})}),`
`,s.jsx(e.li,{children:s.jsx(e.a,{href:"https://learnxinyminutes.com/docs/rust/",children:"Learn X in Y minutes - Rust"})}),`
`]})]})}function t(n={}){const{wrapper:e}={...r(),...n.components};return e?s.jsx(e,{...n,children:s.jsx(i,{...n})}):i(n)}export{t as default,a as frontmatter};
