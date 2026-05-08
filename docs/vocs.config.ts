import React from 'react'
import { defineConfig } from 'vocs'

export default defineConfig({
  title: 'Anchor',
  description: 'Open source Secret Shared Validator client. Built by the community, for the community.',
  logoUrl: '/anchor-logo.png',
  iconUrl: '/anchor-logo.png',

  // Community-focused
  aiCta: false,

  // Enable theme toggle
  themeToggle: true,

  // EXPERIMENT: per-line hanging-indent wrap for `text` code blocks so
  // render_long_help() output (CLI reference pages) wraps without losing
  // Clap's column hierarchy. Wrapping is applied per Shiki `.line` span;
  // a small client script reads each line's leading whitespace and sets
  // padding-left + negative text-indent so wrapped overflow hangs at the
  // original indent column. Remove this `head` block to revert.
  head: () =>
    React.createElement(React.Fragment, null,
      React.createElement('style', {
        dangerouslySetInnerHTML: {
          __html: `
            pre.shiki code {
              white-space: normal !important;
            }
            pre.shiki code .line {
              display: block;
              white-space: pre-wrap;
              word-break: break-word;
              overflow-wrap: anywhere;
            }
          `,
        },
      }),
      React.createElement('script', {
        dangerouslySetInnerHTML: {
          __html: `
            (function () {
              // Extra ch added to every indent level so Clap's 2-space step
              // (header -> name) reads as a clear column on the docs page
              // instead of a barely-visible 2ch nudge.
              var INDENT_BOOST = 2;

              function fix() {
                var lines = document.querySelectorAll('pre.shiki code .line:not([data-indent-fixed])');
                for (var i = 0; i < lines.length; i++) {
                  var line = lines[i];
                  line.setAttribute('data-indent-fixed', '');
                  var inner = line.querySelector('span');
                  var text = (inner || line).textContent;
                  var m = text.match(/^( +)/);
                  var n = m ? m[1].length : 0;
                  if (n > 0) {
                    // Strip leading whitespace from the inner span so we can
                    // express the indent purely as padding (hang-indents
                    // wrapped content automatically). Updating inner.textContent
                    // (not line.textContent) keeps the span structure intact so
                    // vocs' dark-mode color rule still applies.
                    if (inner) inner.textContent = text.replace(/^ +/, '');
                    else line.textContent = text.replace(/^ +/, '');
                    line.style.paddingLeft = (n + INDENT_BOOST) + 'ch';
                  }
                }
              }
              if (document.readyState !== 'loading') fix();
              else document.addEventListener('DOMContentLoaded', fix);
              new MutationObserver(fix).observe(document.documentElement, {
                childList: true, subtree: true
              });
            })();
          `,
        },
      })
    ),

  // Open source theme - matching design3 background and theming
  theme: {
    accentColor: '#00d4aa',
    variables: {
      color: {
        background: { light: '#ffffff', dark: '#0a0a0a' },
        background2: { light: '#f8f9fa', dark: '#111111' },
        background3: { light: '#f3f4f6', dark: '#1a1a1a' },
        text: { light: '#24292f', dark: '#f0f6fc' },
        text2: { light: '#656d76', dark: '#8b949e' },
        textAccent: { light: '#00d4aa', dark: '#00d4aa' },
        border: { light: '#d0d7de', dark: '#333333' },
      }
    }
  },

  // Clean minimal navigation - matching design1 layout
  topNav: [
    { text: 'Documentation', link: '/introduction' },
    { text: 'GitHub', link: 'https://github.com/sigp/anchor' },
    {
      text: 'v1.2.3',
      items: [
        {
          text: 'Releases',
          link: 'https://github.com/sigp/anchor/releases'
        },
        {
          text: 'Contributing',
          link: 'https://github.com/sigp/anchor/blob/main/CONTRIBUTING.md'
        }
      ]
    }
  ],

  sidebar: {
    '/': [
      {
        text: 'Getting Started',
        items: [
          { text: 'Introduction', link: '/introduction' },
          { text: 'Installation', link: '/installation' },
          { text: 'Running an Operator', link: '/running_an_operator' },
          { text: 'Running a Validator on SSV', link: '/running_a_validator_on_ssv' },
        ]
      },
      {
        text: 'Usage & Configuration',
        items: [
          { text: 'Migrate to Anchor', link: '/migrate_to_anchor' },
          {
            text: 'CLI Reference', link: '/cli', collapsed: true,
            items: [
              { text: 'Node', link: '/cli-node' },
              { text: 'Keygen', link: '/cli-keygen' },
              { text: 'KeySplit', link: '/cli-keysplit' },
            ]
          },
          { text: 'Metrics', link: '/metrics' },
          { text: 'Advanced Networking', link: '/advanced_networking' },
          { text: 'FAQs', link: '/faq' },
        ]
      },
      {
        text: 'Development',
        items: [
          { text: 'Development Environment', link: '/development_environment' },
          {
            text: 'Protocol Developers', link: '/protocol_developers', collapsed: true,
            items: [{ text: 'SSV Handshake Protocol', link: '/handshake' }]
          },
          { text: 'Architecture', link: '/architecture' },
          { text: 'Contributing', link: '/contributing' },
        ]
      },
    ]
  },

  socials: [
    {
      icon: 'github',
      link: 'https://github.com/sigp/anchor',
    },
    {
      icon: 'x',
      link: 'https://x.com/sigp_io',
    },
  ],
})
