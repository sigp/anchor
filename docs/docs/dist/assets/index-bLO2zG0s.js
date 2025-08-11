import{u as i,j as e}from"./index-CPZ5_ZSA.js";const t={content:{width:"100%"},layout:"landing",showLogo:!1,title:"Anchor",description:"A highly performant and secure SSV client written in Rust, by Sigma Prime."};function r(a){const s={code:"code",div:"div",p:"p",pre:"pre",span:"span",...i(),...a.components};return e.jsxs(e.Fragment,{children:[e.jsxs("div",{className:"anchor-landing",children:[e.jsx("div",{className:"hero-section",children:e.jsxs("div",{className:"hero-content",children:[e.jsxs("div",{className:"hero-title-container",children:[e.jsx("img",{src:"/anchor-logo.png",alt:"Anchor Logo",className:"logo-image"}),e.jsx("h1",{className:"hero-title",children:"Anchor"})]}),e.jsx("p",{className:"hero-subtitle",children:e.jsxs(s.p,{children:["A highly performant and secure SSV client",e.jsx("br",{}),`
written in Rust, by Sigma Prime.`]})}),e.jsxs("div",{className:"hero-buttons",children:[e.jsx("a",{href:"/installation",className:"btn btn-primary",children:"Get Started"}),e.jsx("a",{href:"/introduction",className:"btn btn-secondary",children:"Documentation"}),e.jsx("a",{href:"https://github.com/sigp/anchor",className:"btn btn-secondary",children:"GitHub"})]}),e.jsx("div",{className:"releases-link",children:e.jsx("a",{href:"https://github.com/sigp/anchor/releases",className:"link-accent",children:e.jsx(s.p,{children:"→ View all releases"})})})]})}),e.jsx("div",{className:"code-section",children:e.jsx("div",{className:"code-example",children:e.jsxs(s.div,{className:"code-group",children:[e.jsx(s.div,{"data-title":"Docker",children:e.jsx(e.Fragment,{children:e.jsx(s.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0","data-title":"Docker","data-lang":"bash",children:e.jsxs(s.code,{children:[e.jsx(s.span,{className:"line",children:e.jsx(s.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"# Pull the latest image"})}),`
`,e.jsxs(s.span,{className:"line",children:[e.jsx(s.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:"docker"}),e.jsx(s.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:" pull"}),e.jsx(s.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:" sigp/anchor:latest"})]}),`
`,e.jsx(s.span,{className:"line","data-empty-line":!0,children:" "}),`
`,e.jsx(s.span,{className:"line",children:e.jsx(s.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"# Run the container"})}),`
`,e.jsxs(s.span,{className:"line",children:[e.jsx(s.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:"docker"}),e.jsx(s.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:" run"}),e.jsx(s.span,{style:{color:"#005CC5","--shiki-dark":"#6CB6FF"},children:" --rm"}),e.jsx(s.span,{style:{color:"#005CC5","--shiki-dark":"#6CB6FF"},children:" -it"}),e.jsx(s.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:" sigp/anchor:latest"}),e.jsx(s.span,{style:{color:"#005CC5","--shiki-dark":"#6CB6FF"},children:" --help"})]})]})})})}),e.jsx(s.div,{"data-title":"From Source",children:e.jsx(e.Fragment,{children:e.jsx(s.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0","data-title":"From Source","data-lang":"bash",children:e.jsxs(s.code,{children:[e.jsx(s.span,{className:"line",children:e.jsx(s.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"# Clone and build"})}),`
`,e.jsxs(s.span,{className:"line",children:[e.jsx(s.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:"git"}),e.jsx(s.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:" clone"}),e.jsx(s.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:" https://github.com/sigp/anchor"})]}),`
`,e.jsxs(s.span,{className:"line",children:[e.jsx(s.span,{style:{color:"#005CC5","--shiki-dark":"#6CB6FF"},children:"cd"}),e.jsx(s.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:" anchor"})]}),`
`,e.jsx(s.span,{className:"line",children:e.jsx(s.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"# Make the binary"})}),`
`,e.jsx(s.span,{className:"line",children:e.jsx(s.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:"make"})}),`
`,e.jsx(s.span,{className:"line",children:e.jsx(s.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"# Make sure ~/.cargo/bin is in your $PATH"})}),`
`,e.jsxs(s.span,{className:"line",children:[e.jsx(s.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:"anchor"}),e.jsx(s.span,{style:{color:"#005CC5","--shiki-dark":"#6CB6FF"},children:" --help"})]})]})})})}),e.jsx(s.div,{"data-title":"From Release",children:e.jsx(e.Fragment,{children:e.jsx(s.pre,{className:"shiki shiki-themes github-light github-dark-dimmed",style:{backgroundColor:"#fff","--shiki-dark-bg":"#22272e",color:"#24292e","--shiki-dark":"#adbac7"},tabIndex:"0","data-title":"From Release","data-lang":"bash",children:e.jsxs(s.code,{children:[e.jsx(s.span,{className:"line",children:e.jsx(s.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"# Specify the platform i.e x86_64-apple-darwin (for apple) x86_64-unknown-linux-gnu.tar.gz"})}),`
`,e.jsxs(s.span,{className:"line",children:[e.jsx(s.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:"wget"}),e.jsx(s.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:" https://github.com/sigp/anchor/releases/download/v0.2.0/anchor-"}),e.jsx(s.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"<"}),e.jsx(s.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:"platfor"}),e.jsx(s.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:"m"}),e.jsx(s.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:">"}),e.jsx(s.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:".tar.gz"})]}),`
`,e.jsx(s.span,{className:"line",children:e.jsx(s.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"# Extract the file"})}),`
`,e.jsxs(s.span,{className:"line",children:[e.jsx(s.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:"tar"}),e.jsx(s.span,{style:{color:"#005CC5","--shiki-dark":"#6CB6FF"},children:" -xvf"}),e.jsx(s.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:" anchor-"}),e.jsx(s.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:"<"}),e.jsx(s.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:"platfor"}),e.jsx(s.span,{style:{color:"#24292E","--shiki-dark":"#ADBAC7"},children:"m"}),e.jsx(s.span,{style:{color:"#D73A49","--shiki-dark":"#F47067"},children:">"}),e.jsx(s.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:".tar.gz"})]}),`
`,e.jsx(s.span,{className:"line",children:e.jsx(s.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"# Make it executable"})}),`
`,e.jsxs(s.span,{className:"line",children:[e.jsx(s.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:"chmod"}),e.jsx(s.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:" +x"}),e.jsx(s.span,{style:{color:"#032F62","--shiki-dark":"#96D0FF"},children:" anchor"})]}),`
`,e.jsx(s.span,{className:"line",children:e.jsx(s.span,{style:{color:"#6A737D","--shiki-dark":"#768390"},children:"# Run it"})}),`
`,e.jsxs(s.span,{className:"line",children:[e.jsx(s.span,{style:{color:"#6F42C1","--shiki-dark":"#F69D50"},children:"./anchor"}),e.jsx(s.span,{style:{color:"#005CC5","--shiki-dark":"#6CB6FF"},children:" --help"})]})]})})})})]})})}),e.jsx("div",{className:"stats-section",children:e.jsxs("div",{className:"stats-row",children:[e.jsxs("div",{className:"stat-item",children:[e.jsx("div",{className:"stat-number",children:"45"}),e.jsx("div",{className:"stat-label",children:"Stars"})]}),e.jsxs("div",{className:"stat-item",children:[e.jsx("div",{className:"stat-number",children:"12"}),e.jsx("div",{className:"stat-label",children:"Contributors"})]}),e.jsxs("div",{className:"stat-item",children:[e.jsx("div",{className:"stat-number",children:"v0.2.0"}),e.jsx("div",{className:"stat-label",children:"Version"})]}),e.jsxs("div",{className:"stat-item",children:[e.jsx("div",{className:"stat-number",children:"Apache"}),e.jsx("div",{className:"stat-label",children:"License"})]})]})}),e.jsx("div",{className:"features-section",children:e.jsxs("div",{className:"features-grid",children:[e.jsxs("div",{className:"feature-card",children:[e.jsx("h3",{children:"Secure and Performant"}),e.jsx("p",{children:"Built with Rust's memory safety and security best practices"})]}),e.jsxs("div",{className:"feature-card",children:[e.jsx("h3",{children:"Community Driven"}),e.jsx("p",{children:"Open development with contributions from the ecosystem"})]}),e.jsxs("div",{className:"feature-card",children:[e.jsx("h3",{children:"Differentially Fuzzed"}),e.jsx("p",{children:`We reguarly fuzz both the Anchor and the go-ssv client for differences and security
vulnerabilities`})]})]})}),e.jsx("div",{className:"footer-section",children:e.jsxs("div",{className:"footer-content",children:[e.jsx("div",{className:"footer-text",children:e.jsxs(s.p,{children:["Built with ❤️ by ",e.jsx("a",{href:"https://sigmaprime.io",className:"footer-link",children:"Sigma Prime"})," and the community"]})}),e.jsxs("div",{className:"footer-links",children:[e.jsx("a",{href:"https://github.com/sigp/anchor/blob/main/LICENSE",className:"footer-link",children:"Apache 2.0 License"}),e.jsx("span",{className:"footer-separator",children:"•"}),e.jsx("a",{href:"https://github.com/sigp/anchor",className:"footer-link",children:"Source Code"}),e.jsx("span",{className:"footer-separator",children:"•"}),e.jsx("a",{href:"/docs/conduct",className:"footer-link",children:"Code of Conduct"})]})]})})]}),`
`,e.jsx("style",{children:`
.anchor-landing {
  max-width: 1200px;
  margin: 0 auto;
  padding: 0 24px;
}

.hero-section {
  text-align: center;
  padding: 0px 0 10px 0;
}

.hero-content {
  max-width: 800px;
  margin: 0 auto;
}

.hero-title-container {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 1rem;
  margin-bottom: 1.5rem;
}

.logo-image {
  height: 50px;
  width: auto;
  filter: brightness(1.1) contrast(1.2);
  transition: all 0.3s ease;
}

.logo-image:hover {
  filter: brightness(1.2) contrast(1.3) drop-shadow(0 0 20px rgba(0, 212, 170, 0.3));
  transform: scale(1.05);
}

.hero-title {
  font-size: 4rem;
  font-weight: 700;
  margin: 0;
  background: linear-gradient(135deg, #00d4aa, #0099cc);
  -webkit-background-clip: text;
  -webkit-text-fill-color: transparent;
  background-clip: text;
  line-height: 1.1;
}

.hero-subtitle {
  font-size: 1.5rem;
  color: #8b949e;
  margin: 0 0 2.5rem 0;
  line-height: 1.6;
  max-width: 600px;
  margin-left: auto;
  margin-right: auto;
  margin-bottom: 2.5rem;
}

.hero-buttons {
  display: flex;
  justify-content: center;
  gap: 1rem;
  flex-wrap: wrap;
}

.btn {
  padding: 0.875rem 2rem;
  border-radius: 8px;
  text-decoration: none;
  font-weight: 600;
  font-size: 0.95rem;
  transition: all 0.3s ease;
  display: inline-block;
  border: 2px solid transparent;
}

.btn-primary {
  background: #00d4aa;
  color: #0a0a0a;
  border-color: #00d4aa;
}

.btn-primary:hover {
  background: #00b89a;
  border-color: #00b89a;
  transform: translateY(-1px);
  box-shadow: 0 4px 12px rgba(0, 212, 170, 0.3);
}

.btn-secondary {
  background: transparent;
  border-color: #00d4aa;
  color: #00d4aa;
}

.btn-secondary:hover {
  background: #00d4aa;
  color: #0a0a0a;
  transform: translateY(-1px);
}

.code-section {
  margin: 50px 0;
  display: flex;
  justify-content: center;
}

.code-example {
  max-width: 600px;
  width: 100%;
}

.stats-section {
  padding: 2rem 0;
  border-top: 1px solid rgba(255, 255, 255, 0.1);
  border-bottom: 1px solid rgba(255, 255, 255, 0.1);
}

.stats-row {
  display: flex;
  justify-content: center;
  gap: 3rem;
  flex-wrap: wrap;
}

.stat-item {
  text-align: center;
}

.stat-number {
  font-size: 1.75rem;
  font-weight: 700;
  color: #00d4aa;
  margin-bottom: 0.25rem;
}

.stat-label {
  font-size: 0.9rem;
  color: #8b949e;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  font-weight: 500;
}

.releases-link {
  text-align: center;
  margin-top: 1.5rem;
}

.link-accent {
  color: #00d4aa;
  text-decoration: none;
  font-size: 0.9rem;
  font-weight: 500;
  transition: color 0.3s ease;
}

.link-accent:hover {
  color: #00b89a;
}

.features-section {
  padding: 80px 0;
}

.features-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
  gap: 2rem;
}

.feature-card {
  background: rgba(255, 255, 255, 0.03);
  border: 1px solid rgba(255, 255, 255, 0.1);
  border-radius: 12px;
  padding: 2rem;
  text-align: center;
  transition: all 0.3s ease;
}

.feature-card:hover {
  background: rgba(255, 255, 255, 0.05);
  border-color: rgba(0, 212, 170, 0.2);
  transform: translateY(-2px);
}

.feature-card h3 {
  font-size: 1.25rem;
  font-weight: 600;
  color: #f0f6fc;
  margin: 0 0 1rem 0;
}

.feature-card p {
  font-size: 0.95rem;
  color: #8b949e;
  line-height: 1.6;
  margin: 0;
}

.footer-section {
  border-top: 1px solid rgba(255, 255, 255, 0.1);
  padding: 2rem 0;
  text-align: center;
}

.footer-content {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.5rem;
}

.footer-text {
  color: #8b949e;
  font-size: 0.95rem;
}

.footer-links {
  display: flex;
  align-items: center;
  gap: 1rem;
  flex-wrap: wrap;
  justify-content: center;
}

.footer-link {
  color: #00d4aa;
  text-decoration: none;
  font-size: 0.9rem;
  transition: color 0.3s ease;
}

.footer-link:hover {
  color: #00b89a;
}

.footer-separator {
  color: #8b949e;
}

@media (max-width: 768px) {
  .anchor-landing {
    padding: 0 16px;
  }

  .hero-section {
    padding: 0px 0 10px 0;
  }

  .hero-title-container {
    gap: 0.75rem;
  }

  .logo-image {
    height: 40px;
  }

  .hero-title {
    font-size: 2.5rem;
  }

  .hero-subtitle {
    font-size: 1.25rem;
  }

  .hero-buttons {
    flex-direction: column;
    align-items: center;
  }

  .btn {
    width: 100%;
    max-width: 300px;
    text-align: center;
  }

  .features-grid {
    grid-template-columns: 1fr;
  }

  .stats-row {
    flex-wrap: wrap;
    gap: 1.5rem;
    justify-content: center;
  }

  .footer-links {
    flex-direction: column;
    gap: 0.5rem;
  }
}
`})]})}function l(a={}){const{wrapper:s}={...i(),...a.components};return s?e.jsx(s,{...a,children:e.jsx(r,{...a})}):r(a)}export{l as default,t as frontmatter};
