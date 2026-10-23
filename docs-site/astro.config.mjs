import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLlmsTxt from 'starlight-llms-txt';

export default defineConfig({
  site: 'https://docs.visiconvert.com',
  integrations: [
    starlight({
      title: 'VisiConvert Docs',
      description: 'Self-hosted call tracking and lead attribution for service businesses.',
      logo: {
        light: './src/assets/visiconvert-wordmark.svg',
        dark: './src/assets/visiconvert-wordmark-light.svg',
        replacesTitle: true,
      },
      favicon: '/favicon.svg',
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/VisiOffice/visiconvert' },
      ],
      editLink: { baseUrl: 'https://github.com/VisiOffice/visiconvert/edit/workspace-api-keys/docs-site/' },
      customCss: ['@fontsource-variable/manrope', './src/styles/theme.css'],
      plugins: [starlightLlmsTxt()],
      sidebar: [
        { label: 'Get started', items: ['start/introduction', 'start/self-host', 'start/connect-twilio'] },
        { label: 'Concepts', items: ['concepts/leads-and-bookings'] },
        { label: 'Command line', items: ['cli/overview', 'reference/cli'] },
        { label: 'AI agents', items: ['agents/mcp', 'reference/mcp'] },
        { label: 'API', items: ['api/public-api', 'api/overview'] },
      ],
    }),
  ],
});
