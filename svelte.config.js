import adapter from "@sveltejs/adapter-static";

/** @type {import('@sveltejs/kit').Config} */
const config = {
  kit: {
    adapter: adapter({
      fallback: "index.html",
    }),
    // $lib is built-in; $messages aliased here so the generated tsconfig + Vite both pick it up
    alias: {
      $messages: "messages",
    },
    paths: {
      relative: false,
    },
  },
};

export default config;
