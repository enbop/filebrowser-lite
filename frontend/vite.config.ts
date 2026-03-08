import path from "node:path";
import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import VueI18nPlugin from "@intlify/unplugin-vue-i18n/vite";
import legacy from "@vitejs/plugin-legacy";
import { compression } from "vite-plugin-compression2";

const plugins = [
  vue(),
  VueI18nPlugin({
    include: [path.resolve(__dirname, "./src/i18n/**/*.json")],
  }),
  legacy({
    targets: ["defaults"],
  }),
  compression({ include: /\.js$/, deleteOriginalAssets: false }),
];

const liteBuild = process.env.FILEBROWSER_LITE_WASI === "1";

const resolve = {
  alias: {
    "@/": `${path.resolve(__dirname, "src")}/`,
  },
};

const build = {
  rollupOptions: {
    input: {
      index: path.resolve(
        __dirname,
        liteBuild ? "./index.html" : "./public/index.html"
      ),
    },
    output: {
      manualChunks: (id: string) => {
        if (id.includes("dayjs/")) {
          return "dayjs";
        }

        if (id.includes("i18n/")) {
          return "i18n";
        }
      },
    },
  },
};

export default defineConfig(({ command }) => {
  if (command === "serve") {
    return {
      plugins,
      resolve,
      server: {
        proxy: {
          "/api/command": {
            target: "ws://127.0.0.1:8080",
            ws: true,
          },
          "/api": "http://127.0.0.1:8080",
        },
      },
    };
  }

  if (liteBuild) {
    return {
      plugins,
      resolve,
      base: "/",
      build,
    };
  }

  return {
    plugins,
    resolve,
    base: "",
    build,
    experimental: {
      renderBuiltUrl(filename, { hostType }) {
        if (hostType === "js") {
          return { runtime: `window.__prependStaticUrl("${filename}")` };
        }

        if (hostType === "html") {
          return `[{[ .StaticURL ]}]/${filename}`;
        }

        return { relative: true };
      },
    },
  };
});
