// reused-from: binst-pilot-webapp@dda2d1a:api/indexer.js (2026-11-16) — adapted: none (verbatim)
// Same-origin proxy to the UniSat Open API inscription indexer on testnet4.
// Xverse dropped testnet4 in 2026 (its hosts now 503 or redirect to signet).
// The optional API key lives only in the UNISAT_API_KEY environment variable,
// never in the WASM bundle. Only the read-only GET paths the webapp uses are
// forwarded, and responses are cached at the edge to save the daily quota.

const UPSTREAM = "https://open-api-testnet4.unisat.io";

const INSCRIPTION_ID = "[0-9a-f]{64}i\\d{1,6}";
const LIST = new RegExp("^/v1/indexer/address/[a-zA-Z0-9]{14,90}/inscription-data$");
const INFO = new RegExp(`^/v1/indexer/inscription/info/${INSCRIPTION_ID}$`);
const CONTENT = new RegExp(`^/v1/indexer/inscription/content/${INSCRIPTION_ID}$`);

const ALLOWED_QUERY = new Set(["cursor", "size"]);
const MAX_PAGE_SIZE = 100;

// The webapp only reads BINST JSON bodies. Anything else (HTML, SVG, images)
// is refused so an attacker's inscription can never render on this origin.
const CONTENT_TYPES = [/^application\/json\b/i, /^text\/plain\b/i];

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// UniSat answers keyless bursts with 403 {code:-2005} and an exhausted quota
// with 403 {code:-2004}. Retry only the burst limit.
async function fetchUpstream(url, key) {
  const headers = { accept: "application/json" };
  if (key) headers.authorization = `Bearer ${key}`;
  for (let attempt = 0; ; attempt++) {
    const upstream = await fetch(url, { headers });
    if (upstream.status !== 403 || attempt >= 2) return upstream;
    const body = await upstream.clone().json().catch(() => null);
    if (!body || body.code !== -2005) return upstream;
    await sleep(400 * (attempt + 1));
  }
}

module.exports = async (req, res) => {
  if (req.method !== "GET") {
    res.status(405).json({ error: "method not allowed" });
    return;
  }

  const url = new URL(req.url, "http://localhost");
  const path = url.searchParams.get("path") || "";
  const isContent = CONTENT.test(path);
  if (!isContent && !LIST.test(path) && !INFO.test(path)) {
    res.status(404).json({ error: "path not allowed" });
    return;
  }
  const query = new URLSearchParams();
  for (const [k, v] of url.searchParams) {
    if (!ALLOWED_QUERY.has(k) || !/^\d{1,6}$/.test(v)) continue;
    query.set(k, k === "size" ? String(Math.min(Number(v), MAX_PAGE_SIZE)) : v);
  }

  let upstream;
  try {
    upstream = await fetchUpstream(
      `${UPSTREAM}${path}${query.size ? `?${query}` : ""}`,
      process.env.UNISAT_API_KEY,
    );
  } catch (e) {
    res.setHeader("cache-control", "no-store");
    res.status(502).json({ error: "indexer unavailable" });
    return;
  }

  if (upstream.status === 403 || upstream.status === 401) {
    // -2004/-2005 are rate or quota limits; anything else (e.g. a rejected key)
    // is a configuration problem, reported with UniSat's own message.
    const body = await upstream.json().catch(() => null);
    const code = body && body.code;
    const limited = code === -2004 || code === -2005;
    res.setHeader("cache-control", "no-store");
    res.status(limited ? 429 : 502).json({
      error: limited ? "indexer quota exhausted" : "indexer rejected the request",
      code,
      msg: body && typeof body.msg === "string" ? body.msg.slice(0, 200) : undefined,
    });
    return;
  }

  if (isContent) {
    const type = upstream.headers.get("content-type") || "";
    if (upstream.ok && !CONTENT_TYPES.some((re) => re.test(type))) {
      res.setHeader("cache-control", "public, s-maxage=86400");
      res.status(415).json({ error: "content type not served", type });
      return;
    }
    const body = Buffer.from(await upstream.arrayBuffer());
    res.setHeader("content-type", type || "application/octet-stream");
    res.setHeader("content-security-policy", "sandbox; default-src 'none'");
    // Inscription content never changes once confirmed.
    res.setHeader("cache-control", upstream.ok ? "public, max-age=86400, s-maxage=31536000, immutable" : "no-store");
    res.status(upstream.status).send(body);
    return;
  }

  // JSON endpoints wrap the payload as {code, msg, data}; code != 0 is an error.
  const text = await upstream.text();
  let json = null;
  try { json = JSON.parse(text); } catch (e) { /* non-JSON upstream error */ }
  if (!upstream.ok || !json || json.code !== 0) {
    res.setHeader("cache-control", "no-store");
    res.status(upstream.ok ? 502 : upstream.status).json({
      error: (json && json.msg) || "indexer error",
      code: json && json.code,
    });
    return;
  }
  res.setHeader("content-type", "application/json");
  res.setHeader("cache-control", "public, max-age=10, s-maxage=30, stale-while-revalidate=300");
  res.status(200).send(text);
};
