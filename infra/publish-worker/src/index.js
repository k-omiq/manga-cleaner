// PUT /<key>  with  Authorization: Bearer $PUBLISH_TOKEN  → writes the body to
// the cleaner-updates bucket under <key>. Nothing else is served here; reads
// go to https://cleaner.komiq.cc/<key>.
//
// Bodies stream straight into R2. Workers cap a request body at 100 MB on the
// free plan, which is far above any installer this app ships; a larger file
// fails with 413 from the platform rather than silently truncating.

function timingSafeEqual(a, b) {
  const enc = new TextEncoder();
  const x = enc.encode(a);
  const y = enc.encode(b);
  if (x.byteLength !== y.byteLength) return false;
  return crypto.subtle.timingSafeEqual(x, y);
}

export default {
  async fetch(request, env) {
    if (request.method !== "PUT") {
      return new Response("method not allowed", { status: 405, headers: { allow: "PUT" } });
    }
    const auth = request.headers.get("authorization") || "";
    const token = auth.startsWith("Bearer ") ? auth.slice(7) : "";
    if (!env.PUBLISH_TOKEN || !token || !timingSafeEqual(token, env.PUBLISH_TOKEN)) {
      return new Response("unauthorized", { status: 401 });
    }
    const key = decodeURIComponent(new URL(request.url).pathname.replace(/^\/+/, ""));
    if (!key || key.includes("..")) {
      return new Response("bad key", { status: 400 });
    }
    const contentType = request.headers.get("content-type") || "application/octet-stream";
    const object = await env.UPDATES.put(key, request.body, {
      httpMetadata: { contentType },
    });
    return Response.json({ key: object.key, size: object.size, etag: object.httpEtag });
  },
};
