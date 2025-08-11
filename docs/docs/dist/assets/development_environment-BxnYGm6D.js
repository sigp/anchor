import{u as i,j as e}from"./index-CPZ5_ZSA.js";const l={title:"Development Environment",description:"undefined"};function t(s){const n={a:"a",code:"code",div:"div",h1:"h1",h2:"h2",header:"header",li:"li",p:"p",pre:"pre",span:"span",strong:"strong",ul:"ul",...i(),...s.components};return e.jsxs(e.Fragment,{children:[e.jsx(n.header,{children:e.jsxs(n.h1,{id:"development-environment",children:["Development Environment",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#development-environment",children:e.jsx(n.div,{"data-autolink-icon":!0})})]})}),`
`,e.jsx(n.p,{children:`Most Anchor developers work on Linux or MacOS, however Windows should still
be suitable.`}),`
`,e.jsxs(n.p,{children:["First, follow the ",e.jsx(n.a,{href:"/installation",children:e.jsx(n.code,{children:"Installation Guide"})}),` to install
Anchor. This will install Anchor to your `,e.jsx(n.code,{children:"PATH"}),`, which is not
particularly useful for development but still a good way to ensure you have the
base dependencies.`]}),`
`,e.jsx(n.p,{children:"The additional requirements for developers are:"}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsxs(n.li,{children:[e.jsx(n.a,{href:"https://www.docker.com/",children:e.jsx(n.code,{children:"docker"})}),". Some tests need docker installed and ",e.jsx(n.strong,{children:"running"}),"."]}),`
`]}),`
`,e.jsxs(n.h2,{id:"using-make",children:["Using ",e.jsx(n.code,{children:"make"}),e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#using-make",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.p,{children:["Commands to run the test suite are available via the ",e.jsx(n.code,{children:"Makefile"}),` in the
project root for the benefit of CI/CD. We list some of these commands below so
you can run them locally and avoid CI failures:`]}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsxs(n.li,{children:[e.jsx(n.code,{children:"$ make cargo-fmt"}),": (fast) runs a Rust code formatting check."]}),`
`,e.jsxs(n.li,{children:[e.jsx(n.code,{children:"$ make lint"}),": (fast) runs a Rust code linter."]}),`
`,e.jsxs(n.li,{children:[e.jsx(n.code,{children:"$ make test"}),": (medium) runs unit tests across the whole project."]}),`
`]}),`
`,e.jsxs(n.h2,{id:"testing",children:["Testing",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#testing",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(n.p,{children:["As with most other Rust projects, Anchor uses ",e.jsx(n.code,{children:"cargo test"}),` for unit and
integration tests. For example, to test the `,e.jsx(n.code,{children:"qbft"})," crate run:"]}),`
`,e.jsx(e.Fragment,{children:e.jsx(n.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0",children:e.jsxs(n.code,{children:[e.jsxs(n.span,{className:"line",children:[e.jsx(n.span,{style:{color:"#005CC5","--shiki-dark":"#6CB6FF"},children:"cd"}),e.jsx(n.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:" src/qbft"})]}),`
`,e.jsxs(n.span,{className:"line",children:[e.jsx(n.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:"cargo"}),e.jsx(n.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:" test"})]})]})})}),`
`,e.jsxs(n.h2,{id:"local-testnets",children:["Local Testnets",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#local-testnets",children:e.jsx(n.div,{"data-autolink-icon":!0})})]}),`
`,e.jsx(n.p,{children:`During development and testing it can be useful to start a small, local
testnet.`}),`
`,e.jsx(n.p,{children:"Testnet scripts will be built as the project develops."})]})}function o(s={}){const{wrapper:n}={...i(),...s.components};return n?e.jsx(n,{...s,children:e.jsx(t,{...s})}):t(s)}export{o as default,l as frontmatter};
