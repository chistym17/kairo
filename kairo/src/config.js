/* Site public URL for the install command shown on the homepage.
 *
 * Priority used by the page:
 *   1. window.KAIRO_SITE_URL (this file / deploy env) — if non-empty
 *   2. window.location.origin (wherever the site is hosted)
 *   3. <meta name="kairo-site-url">
 *   4. https://kairo.so
 *
 * Deploy example (inject from env):
 *   KAIRO_SITE_URL=https://kairo.so
 *   printf 'window.KAIRO_SITE_URL = %s;\n' "$(node -p 'JSON.stringify(process.env.KAIRO_SITE_URL||"")')" > config.js
 *
 * Leave empty to auto-use the live page origin
 * (curl becomes: curl -fsSL https://<this-site>/install.sh | sh).
 */
window.KAIRO_SITE_URL = "";

