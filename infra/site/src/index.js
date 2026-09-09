// Read side of https://cleaner.komiq.cc.
//
// Static files under public/ (the landing page, the icon, the demo video) are
// served by Workers Assets before this handler runs. Everything else is a
// release object in the cleaner-updates R2 bucket: latest.json, which installed
// apps poll for updates, and releases/v<version>/<file> for the installers and
// their signatures.
//
// Writes do not happen here. GitHub Actions PUTs to the separate publish worker
// at https://cleaner-publish.komiq.cc, which holds the only write token.

// R2 reports the served range as one of {offset,length?} | {offset?,length} |
// {suffix}. Normalise all three to an inclusive start/end pair so the 206 can
// carry a correct content-range and content-length.
function resolveRange(range, size) {
  if ("suffix" in range) {
    const start = Math.max(0, size - range.suffix);
    return { start, end: size - 1 };
  }
  const start = range.offset ?? 0;
  const end = range.length === undefined ? size - 1 : start + range.length - 1;
  return { start, end: Math.min(end, size - 1) };
}

export default {
  async fetch(request, env) {
    if (request.method !== "GET" && request.method !== "HEAD") {
      return new Response("method not allowed", {
        status: 405,
        headers: { allow: "GET, HEAD" },
      });
    }

    const url = new URL(request.url);
    const key = decodeURIComponent(url.pathname.replace(/^\/+/, ""));
    // An empty key is the bucket root, and ".." is a traversal attempt. Neither
    // names an object; both are a plain miss.
    if (!key || key.includes("..")) {
      return new Response("not found", { status: 404 });
    }

    const object = await env.UPDATES.get(key, {
      range: request.headers,
      onlyIf: request.headers,
    });
    if (!object) {
      return new Response("not found", { status: 404 });
    }

    const headers = new Headers();
    object.writeHttpMetadata(headers);
    headers.set("etag", object.httpEtag);
    headers.set("accept-ranges", "bytes");
    // Release objects are immutable once published; the manifest is not.
    headers.set(
      "cache-control",
      key === "latest.json"
        ? "public, max-age=60"
        : "public, max-age=31536000, immutable",
    );

    // A failed precondition (If-None-Match on an unchanged object) returns an
    // R2Object with no body.
    if (!("body" in object)) {
      return new Response(null, { status: 304, headers });
    }

    let status = 200;
    if (object.range && request.headers.has("range")) {
      const { start, end } = resolveRange(object.range, object.size);
      headers.set("content-range", `bytes ${start}-${end}/${object.size}`);
      headers.set("content-length", String(end - start + 1));
      status = 206;
    } else {
      headers.set("content-length", String(object.size));
    }

    return new Response(request.method === "HEAD" ? null : object.body, {
      status,
      headers,
    });
  },
};
