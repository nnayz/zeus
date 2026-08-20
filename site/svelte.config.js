import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';
import { mdsvex } from 'mdsvex';

const base = process.env.BASE_PATH ?? '';
const markdownExtensions = ['.md'];

/** @type {import('@sveltejs/kit').Config} */
const config = {
  extensions: ['.svelte', ...markdownExtensions],
  preprocess: [vitePreprocess(), mdsvex({ extensions: markdownExtensions })],
  kit: {
    paths: {
      base
    },
    adapter: adapter({
      pages: 'build',
      assets: 'build',
      strict: true
    })
  }
};

export default config;
