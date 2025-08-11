import{u as o,j as e}from"./index-CPZ5_ZSA.js";const i={title:"Advanced Networking",description:"undefined"};function r(t){const n={a:"a",code:"code",div:"div",h1:"h1",header:"header",li:"li",p:"p",ul:"ul",...o(),...t.components};return e.jsxs(e.Fragment,{children:[e.jsx(n.header,{children:e.jsxs(n.h1,{id:"advanced-networking",children:["Advanced Networking",e.jsx(n.a,{"aria-hidden":"true",tabIndex:"-1",href:"#advanced-networking",children:e.jsx(n.div,{"data-autolink-icon":!0})})]})}),`
`,e.jsxs(n.p,{children:[`Anchor's networking stack is closely based on Lighthouse's. We refer to
`,e.jsx(n.a,{href:"https://lighthouse-book.sigmaprime.io/advanced_networking.html",children:"Lighthouse's page on Advanced Networking"}),`,
but want to outline several important differences:`]}),`
`,e.jsxs(n.ul,{children:[`
`,e.jsx(n.li,{children:"Currently, Anchor does not support UPnP."}),`
`,e.jsx(n.li,{children:"Anchor uses ports 12001 (UDP), 13001 (TCP), and 12002 (UDP) by default."}),`
`,e.jsxs(n.li,{children:[`Anchor does not yet support ENR auto-update - we therefore recommend manually setting publicly reachable ports via the
`,e.jsx(n.code,{children:"--enr*-port"})," CLI parameters to advertise your node as reachable on the network."]}),`
`]})]})}function a(t={}){const{wrapper:n}={...o(),...t.components};return n?e.jsx(n,{...t,children:e.jsx(r,{...t})}):r(t)}export{a as default,i as frontmatter};
