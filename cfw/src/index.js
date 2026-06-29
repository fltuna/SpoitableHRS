// SpoitableHRS Update Server — Cloudflare Worker

const CORS = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Methods": "GET, OPTIONS",
  "Access-Control-Allow-Headers": "Content-Type",
};

export default {
  async fetch(request, env) {
    if (request.method === "OPTIONS") {
      return new Response(null, { status: 204, headers: CORS });
    }

    const url = new URL(request.url);
    const path = url.pathname;

    if (path === "/health") {
      return Response.json({ status: "ok", version: env.CURRENT_VERSION }, { headers: CORS });
    }

    const match = path.match(/^\/update\/([^/]+)\/([^/]+)$/);
    if (!match) {
      return new Response("Not Found", { status: 404, headers: CORS });
    }

    const [, target, currentVersion] = match;

    const latest = await env.UPDATES.get("latest", "json");
    if (!latest) {
      return new Response(null, { status: 204, headers: CORS });
    }

    if (latest.version === currentVersion || !isNewer(latest.version, currentVersion)) {
      return new Response(null, { status: 204, headers: CORS });
    }

    const platform = latest.platforms?.[target];
    if (!platform) {
      return new Response(null, { status: 204, headers: CORS });
    }

    return Response.json({
      version: latest.version,
      notes: latest.notes || "",
      pub_date: latest.pub_date || new Date().toISOString(),
      url: platform.url,
      signature: platform.signature,
    }, { headers: CORS });
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
