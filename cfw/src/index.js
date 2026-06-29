// SpoitableHRS Update Server — Cloudflare Worker
//
// Tauri updater checks: GET /update/{target}/{current_version}
//
// KV "UPDATES" stores:
//   key: "latest" → JSON { version, notes, pub_date, platforms: { "windows-x86_64": { url, signature } } }
//
// To publish an update, put the manifest in KV:
//   wrangler kv:key put --binding UPDATES "latest" '{ "version": "0.2.0", ... }'

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const path = url.pathname;

    // Health check
    if (path === "/health") {
      return Response.json({ status: "ok", version: env.CURRENT_VERSION });
    }

    // Tauri updater endpoint: /update/{target}/{current_version}
    const match = path.match(/^\/update\/([^/]+)\/([^/]+)$/);
    if (!match) {
      return new Response("Not Found", { status: 404 });
    }

    const [, target, currentVersion] = match;

    const latest = await env.UPDATES.get("latest", "json");
    if (!latest) {
      return new Response(null, { status: 204 });
    }

    // No update needed if already on latest
    if (latest.version === currentVersion || !isNewer(latest.version, currentVersion)) {
      return new Response(null, { status: 204 });
    }

    // Check if platform is supported
    const platform = latest.platforms?.[target];
    if (!platform) {
      return new Response(null, { status: 204 });
    }

    return Response.json({
      version: latest.version,
      notes: latest.notes || "",
      pub_date: latest.pub_date || new Date().toISOString(),
      url: platform.url,
      signature: platform.signature,
    });
  },
};

function isNewer(latest, current) {
  const parse = (v) => v.replace(/^v/, "").split(".").map(Number);
  const [lM, lm, lp] = parse(latest);
  const [cM, cm, cp] = parse(current);
  if (lM !== cM) return lM > cM;
  if (lm !== cm) return lm > cm;
  return lp > cp;
}
