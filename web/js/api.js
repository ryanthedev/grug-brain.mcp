/**
 * HTTP helpers. Every method returns {ok, data, error} and never throws.
 *
 * Security: no user-controlled data is interpolated into HTML here.
 * CSRF: write methods always send X-Grug-Client: web.
 */
export const api = {
  /** Fetch JSON from the grug API. Returns {ok, data, error}. */
  async get(path) {
    try {
      const resp = await fetch(path);
      if (!resp.ok) {
        let msg = `HTTP ${resp.status}`;
        try { const j = await resp.json(); msg = j.error || msg; } catch (_) {}
        return { ok: false, error: msg };
      }
      const data = await resp.json();
      return { ok: true, data };
    } catch (e) {
      return { ok: false, error: e.message || "network error" };
    }
  },

  async brains() { return this.get("/api/brains"); },
  async memories(brain) {
    return this.get(`/api/memories?brain=${encodeURIComponent(brain)}`);
  },
  async memory(brain, category, path) {
    return this.get(
      `/api/memory/${encodeURIComponent(brain)}/${encodeURIComponent(category)}/${encodeURIComponent(path)}`
    );
  },
  async graph(brain) {
    return this.get(`/api/graph?brain=${encodeURIComponent(brain)}&mode=global`);
  },

  // Read-only endpoints.
  async tags(brain) {
    const q = brain ? `?brain=${encodeURIComponent(brain)}` : "";
    return this.get(`/api/tags${q}`);
  },
  async backlinks(brain, path) {
    return this.get(
      `/api/backlinks?brain=${encodeURIComponent(brain)}&path=${encodeURIComponent(path)}`
    );
  },
  async graphLocal(brain, path, hops) {
    return this.get(
      `/api/graph/local?brain=${encodeURIComponent(brain)}&path=${encodeURIComponent(path)}&hops=${encodeURIComponent(hops|0)}`
    );
  },

  /**
   * PUT JSON to a path with required If-Match ETag header.
   * Returns {ok, status, data, error}. Never throws. Always sends the
   * X-Grug-Client header required by the server CSRF middleware.
   */
  async put(path, payload, etag) {
    try {
      const resp = await fetch(path, {
        method: "PUT",
        headers: {
          "Content-Type": "application/json",
          "X-Grug-Client": "web",
          "If-Match": String(etag),
        },
        body: JSON.stringify(payload),
      });
      let data = null;
      try { data = await resp.json(); } catch (_) {}
      if (!resp.ok) {
        const err = (data && data.error) || `HTTP ${resp.status}`;
        return { ok: false, status: resp.status, data, error: err };
      }
      return { ok: true, status: resp.status, data };
    } catch (e) {
      return { ok: false, status: 0, error: e.message || "network error" };
    }
  },

  /** PUT helper specialized for the memory write route. */
  async writeMemory(brain, category, path, body, frontmatter, etag) {
    const url = `/api/memory/${encodeURIComponent(brain)}/${encodeURIComponent(category)}/${encodeURIComponent(path)}`;
    return this.put(url, { body, frontmatter }, etag);
  },

  /**
   * POST JSON. Always sends X-Grug-Client: web (CSRF middleware requirement).
   * Returns {ok, status, data, error}. Never throws.
   */
  async post(path, payload) {
    try {
      const resp = await fetch(path, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "X-Grug-Client": "web",
        },
        body: JSON.stringify(payload || {}),
      });
      let data = null;
      try { data = await resp.json(); } catch (_) {}
      if (!resp.ok) {
        const err = (data && data.error) || `HTTP ${resp.status}`;
        return { ok: false, status: resp.status, data, error: err };
      }
      return { ok: true, status: resp.status, data };
    } catch (e) {
      return { ok: false, status: 0, error: e.message || "network error" };
    }
  },

  /**
   * DELETE. 204 returns {ok:true} with no data. CSRF header required.
   */
  async delete(path) {
    try {
      const resp = await fetch(path, {
        method: "DELETE",
        headers: { "X-Grug-Client": "web" },
      });
      if (!resp.ok) {
        let data = null;
        try { data = await resp.json(); } catch (_) {}
        const err = (data && data.error) || `HTTP ${resp.status}`;
        return { ok: false, status: resp.status, data, error: err };
      }
      return { ok: true, status: resp.status, data: null };
    } catch (e) {
      return { ok: false, status: 0, error: e.message || "network error" };
    }
  },
};
