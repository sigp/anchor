import React from 'react'
import { defineConfig } from 'vocs'

export default defineConfig({
  title: 'Anchor',
  description: 'Open source Secret Shared Validator client. Built by the community, for the community.',
  logoUrl: '/anchor-logo.png',
  iconUrl: '/anchor-logo.png',

  // Community-focused
  aiCta: false,

  // Open source theme - matching design3 background and theming
  theme: {
    accentColor: '#00d4aa',
    colorScheme: 'dark',
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
      text: 'v0.2.0',
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
          { text: 'What is SSV?', link: '/what_is_ssv' },
          { text: 'Installation', link: '/installation' },
          { text: 'Running an Operator', link: '/running_an_operator' },
        ]
      },
      {
        text: 'Usage & Configuration',
        items: [
          { text: 'CLI Reference', link: '/cli', collapsed: true,
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
          { text: 'Protocol Developers', link: '/protocol_developers', collapsed: true,
            items: [ {text: 'SSV Handshake Protocol', link: '/handshake' } ] },
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

  editLink: {
    pattern: "https://github.com/sigp/anchor/edit/unstable/book/docs/pages/:path",
    text: "Edit this page"
  },
})
