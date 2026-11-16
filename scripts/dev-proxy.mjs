// Local stand-in for the Vercel function at /api/indexer.
//
//   node scripts/dev-proxy.mjs              (then `trunk serve` in another tab)
//
// With UNISAT_API_KEY in the environment it runs api/indexer.js itself, like
// Vercel does. Without a key, set INDEXER_UPSTREAM to the origin of a deployed
// copy of this app and requests are forwarded to its /api/indexer; UniSat's
// keyless quota runs out within minutes, so one of the two is needed.
// FAIL=429 (or any status) makes every request fail, to test the error UI.

import http from "node:http";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const handler = require("../api/indexer.js");

const PORT = Number(process.env.PORT || 8787);
const FAIL = process.env.FAIL ? Number(process.env.FAIL) : 0;
const UPSTREAM = (process.env.INDEXER_UPSTREAM || "").replace(/\/+$/, "");
const local = Boolean(process.env.UNISAT_API_KEY);

function vercelResponse(res) {
  res.status = (code) => { res.statusCode = code; return res; };
  res.json = (obj) => {
    res.setHeader("content-type", "application/json");
    res.end(JSON.stringify(obj));
  };
  res.send = (body) => res.end(body);
  return res;
}

async function forward(req, res) {
  const upstream = await fetch(`${UPSTREAM}${req.url}`, { headers: { accept: "application/json" } });
  res.statusCode = upstream.status;
  for (const h of ["content-type", "cache-control"]) {
    const v = upstream.headers.get(h);
    if (v) res.setHeader(h, v);
  }
  res.end(Buffer.from(await upstream.arrayBuffer()));
}

http
  .createServer(async (req, res) => {
    const url = new URL(req.url, "http://localhost");
    if (!url.pathname.startsWith("/api/indexer/")) {
      res.statusCode = 404;
      res.end("not found");
      return;
    }
    if (FAIL) {
      vercelResponse(res).status(FAIL).json({ error: "forced failure (FAIL env)" });
      return;
    }
    if (!local && !UPSTREAM) {
      vercelResponse(res).status(502).json({ error: "dev proxy: set UNISAT_API_KEY or INDEXER_UPSTREAM" });
      return;
    }
    try {
      if (!local) {
        await forward(req, res);
        return;
      }
      // Same rewrite as vercel.json: /api/indexer/<path> → /api/indexer?path=/<path>
      const query = new URLSearchParams(url.searchParams);
      query.set("path", url.pathname.slice("/api/indexer".length));
      req.url = `/api/indexer?${query}`;
      await handler(req, vercelResponse(res));
    } catch (e) {
      res.statusCode = 502;
      res.end(JSON.stringify({ error: "dev proxy error" }));
    }
  })
  .listen(PORT, "127.0.0.1", () => {
    const target = local ? "UniSat (local key)" : UPSTREAM || "nothing: set UNISAT_API_KEY or INDEXER_UPSTREAM";
    console.log(`[dev-proxy] http://127.0.0.1:${PORT}/api/indexer/ → ${target}${FAIL ? ` · failing with ${FAIL}` : ""}`);
  });
