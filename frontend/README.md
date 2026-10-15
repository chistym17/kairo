# Frontend / product site

Static homepage for [kairo.so](https://kairo.so).

## Files

| File | Purpose |
|------|---------|
| `kairo.html` | Homepage |
| `install.sh` | Served at `/install.sh` so the curl command works from the site |
| `config.js` | Optional `KAIRO_SITE_URL` override for the install command |

## Install command

The page builds this automatically:

```bash
curl -fsSL https://<site>/install.sh | sh
```

Set the site URL via env when deploying (optional — otherwise it uses the page origin):

```bash
export KAIRO_SITE_URL=https://kairo.so
printf 'window.KAIRO_SITE_URL = %s;\n' "$(node -p 'JSON.stringify(process.env.KAIRO_SITE_URL || "")')" > config.js
```

Or edit `config.js` / the `<meta name="kairo-site-url">` tag in `kairo.html`.

## Deploy checklist

1. Publish `kairo.html` as the site index (or rename to `index.html` on the host)
2. Publish `install.sh` and `config.js` at the site root
3. Keep `frontend/install.sh` in sync with the repo-root `install.sh`
