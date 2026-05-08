import { build } from "vite";

process.env.CI = "1";

await build({
  clearScreen: false,
});
