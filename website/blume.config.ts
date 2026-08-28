import { defineConfig } from 'blume';

const configuredBase = process.env.BASE_PATH || '/';
const deploymentBase =
  configuredBase === '/'
    ? undefined
    : `/${configuredBase.replace(/^\/+|\/+$/g, '')}`;

export default defineConfig({
  title: 'Mitos',
  description: 'Documentation for the Mitos modal text editor.',
  logo: {
    image: '/icon.png',
    text: 'Mitos',
  },
  content: {
    root: 'src/content/docs',
  },
  deployment: {
    base: deploymentBase,
    output: 'static',
  },
  github: {
    owner: 'mitos-editor',
    repo: 'mitos',
    branch: 'main',
    dir: 'website',
  },
  lastModified: true,
  feedback: false,
  navigation: {
    sidebar: {
      display: 'flat',
      items: [
        { label: 'Overview', href: '/' },
        {
          label: 'Start here',
          items: [
            '/basics',
            '/install',
            '/package-managers',
            '/building-from-source',
          ],
        },
        {
          label: 'Editing with Mitos',
          items: [
            '/usage',
            '/registers',
            '/surround',
            '/textobjects',
            '/syntax-aware-motions',
            '/pickers',
            '/jumplist',
          ],
        },
        {
          label: 'Reference',
          items: [
            '/keymap',
            '/command-line',
            '/commands',
            '/lsp',
            '/lang-support',
            '/workspace-trust',
          ],
        },
        {
          label: 'Configuration',
          items: [
            '/configuration',
            '/editor',
            '/themes',
            '/remapping',
            '/languages',
          ],
        },
        {
          label: 'Help and migration',
          items: [
            '/troubleshooting',
            '/ecosystem',
            '/from-vim',
            '/other-software',
          ],
        },
        {
          label: 'Contributing',
          items: [
            '/guides',
            '/guides/adding-languages',
            '/guides/highlights',
            '/guides/locals',
            '/guides/textobject',
            '/guides/indent',
            '/guides/injection',
            '/guides/tags',
            '/guides/rainbow-bracket-queries',
          ],
        },
      ],
    },
  },
  search: {
    provider: 'orama',
  },
  theme: {
    accent: {
      light: '#5864cf',
      dark: '#a7adff',
    },
    action: '#5864cf',
    background: {
      light: '#fcfcff',
      dark: '#10111a',
    },
    fonts: {
      display: 'inter',
      body: 'inter',
      mono: 'ibm-plex-mono',
    },
    mode: 'system',
    radius: 'lg',
  },
  markdown: {
    code: {
      icons: true,
      wrap: false,
    },
    codeBlocks: {
      theme: {
        light: 'github-light',
        dark: 'github-dark',
      },
    },
    imageZoom: true,
  },
});
