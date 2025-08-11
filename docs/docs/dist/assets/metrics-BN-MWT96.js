import{u as i,j as e}from"./index-CPZ5_ZSA.js";const o={title:"Metrics",description:"undefined"};function r(n){const s={a:"a",code:"code",div:"div",h1:"h1",h2:"h2",header:"header",li:"li",ol:"ol",p:"p",ul:"ul",...i(),...n.components};return e.jsxs(e.Fragment,{children:[e.jsx(s.header,{children:e.jsxs(s.h1,{id:"metrics",children:["Metrics",e.jsx(s.a,{"aria-hidden":"true",tabIndex:"-1",href:"#metrics",children:e.jsx(s.div,{"data-autolink-icon":!0})})]})}),`
`,e.jsx(s.p,{children:`Anchor comes pre-built with a suite of metrics for developers or users to monitor the health
and performance of their node.`}),`
`,e.jsxs(s.p,{children:["They must be enabled at runtime using the ",e.jsx(s.code,{children:"--metrics"})," CLI flag."]}),`
`,e.jsxs(s.h2,{id:"usage",children:["Usage",e.jsx(s.a,{"aria-hidden":"true",tabIndex:"-1",href:"#usage",children:e.jsx(s.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(s.p,{children:["In order to run a metrics server, ",e.jsx(s.code,{children:"docker"})," is required to be installed."]}),`
`,e.jsx(s.p,{children:"Once docker is installed, a metrics server can be run locally via the following steps:"}),`
`,e.jsxs(s.ol,{children:[`
`,e.jsxs(s.li,{children:["Start an anchor node with ",e.jsx(s.code,{children:"$ anchor --metrics"}),`
`,e.jsxs(s.ul,{children:[`
`,e.jsxs(s.li,{children:["The ",e.jsx(s.code,{children:"--metrics"})," flag is required for metrics."]}),`
`]}),`
`]}),`
`,e.jsxs(s.li,{children:["Move into the metrics directory ",e.jsx(s.code,{children:"$ cd metrics"}),"."]}),`
`,e.jsxs(s.li,{children:["Bring the environment up with ",e.jsx(s.code,{children:"$ docker-compose up --build -d"}),"."]}),`
`,e.jsxs(s.li,{children:[`Ensure that Prometheus can access your Anchor node by ensuring it is in
the `,e.jsx(s.code,{children:"UP"})," state at ",e.jsx(s.a,{href:"http://localhost:9090/targets",children:"http://localhost:9090/targets"}),"."]}),`
`,e.jsxs(s.li,{children:["Browse to ",e.jsx(s.a,{href:"http://localhost:3000",children:"http://localhost:3000"}),`
`,e.jsxs(s.ul,{children:[`
`,e.jsxs(s.li,{children:["Username: ",e.jsx(s.code,{children:"admin"})]}),`
`,e.jsxs(s.li,{children:["Password: ",e.jsx(s.code,{children:"changeme"})]}),`
`]}),`
`]}),`
`,e.jsxs(s.li,{children:["Import some dashboards from the ",e.jsx(s.code,{children:"metrics/dashboards"})," directory in this repo:",`
`,e.jsxs(s.ul,{children:[`
`,e.jsxs(s.li,{children:["In the Grafana UI, go to ",e.jsx(s.code,{children:"Dashboards"})," -> ",e.jsx(s.code,{children:"Manage"})," -> ",e.jsx(s.code,{children:"Import"})," -> ",e.jsx(s.code,{children:"Upload .json file"}),"."]}),`
`,e.jsxs(s.li,{children:["The ",e.jsx(s.code,{children:"Summary.json"})," dashboard is a good place to start."]}),`
`]}),`
`]}),`
`]}),`
`,e.jsxs(s.h2,{id:"dashboards",children:["Dashboards",e.jsx(s.a,{"aria-hidden":"true",tabIndex:"-1",href:"#dashboards",children:e.jsx(s.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(s.p,{children:["A suite of dashboards can be found in ",e.jsx(s.code,{children:"metrics/dashboard"}),` directory. The Anchor team will
frequently update these dashboards as new metrics are introduced.`]}),`
`,e.jsx(s.p,{children:`We welcome Pull Requests for any users wishing to add their dashboards to this repository for
others to share.`}),`
`,e.jsxs(s.h2,{id:"scrape-targets",children:["Scrape Targets",e.jsx(s.a,{"aria-hidden":"true",tabIndex:"-1",href:"#scrape-targets",children:e.jsx(s.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(s.p,{children:["Prometheus periodically reads the ",e.jsx(s.code,{children:"metrics/scrape-targets/scrape-targets.json"}),` file. This
file tells Prometheus which endpoints to collect data from. The current file is setup to read
from Anchor on its default metrics port. You can add additional endpoints if you want to collect
metrics from other servers.`]}),`
`,e.jsxs(s.p,{children:[`An example is Lighthouse. You can collect metrics from Anchor and Lighthouse simultaneously if
they are both running. We have an example file `,e.jsx(s.code,{children:"scrape-targets-lighthouse.json"}),` which allows this.
You can replace the `,e.jsx(s.code,{children:"scrape-targets.json"}),` file with the contents of
`,e.jsx(s.code,{children:"scrape-targets-lighthouse.json"}),` if you wish to collect metrics from Anchor and Lighthouse
simultaneously.`]}),`
`,e.jsxs(s.h2,{id:"hosting-publicly",children:["Hosting Publicly",e.jsx(s.a,{"aria-hidden":"true",tabIndex:"-1",href:"#hosting-publicly",children:e.jsx(s.div,{"data-autolink-icon":!0})})]}),`
`,e.jsxs(s.p,{children:[`By default Prometheus and Grafana will only bind to localhost (127.0.0.1), in
order to protect you from accidentally exposing them to the public internet. If
you would like to change this you must edit the `,e.jsx(s.code,{children:"http_addr"})," in ",e.jsx(s.code,{children:"metrics/grafana/grafana.ini"}),"."]})]})}function d(n={}){const{wrapper:s}={...i(),...n.components};return s?e.jsx(s,{...n,children:e.jsx(r,{...n})}):r(n)}export{d as default,o as frontmatter};
