/**
 * Graph panel — sigma.js + graphology renderer.
 *
 * Vendor globals `Sigma` and `graphology` are loaded via <script> in index.html.
 * `router` is not yet extracted; call-sites use `window.router` to avoid
 * forward-reference issues.
 *
 * Exported API:
 *   graph.render(data)              — full render from {nodes, edges}
 *   graph.renderLocal(data, opts)   — N-hop neighborhood render
 *   graph.updateTheme()             — update sigma colors after theme change
 *   graph.refresh()                 — force sigma refresh
 */
export const graph = (() => {
  let sigmaInstance = null;
  let graphData = null; // last-rendered {nodes, edges}

  /** Deterministic category → color from a fixed palette (unchanged from Plan 1). */
  function categoryColor(cat) {
    const PALETTE = [
      "#7aa2f7","#9ece6a","#e0af68","#bb9af7","#f7768e",
      "#73daca","#0db9d7","#ff9e64","#c3e88d","#89ddff",
    ];
    let h = 5381;
    for (let i = 0; i < cat.length; i++) h = ((h << 5) + h) + cat.charCodeAt(i) | 0;
    return PALETTE[Math.abs(h) % PALETTE.length];
  }

  /**
   * Place nodes in category clusters arranged radially — O(n), instant for
   * large graphs. Each category occupies a circular sub-cluster on the ring.
   */
  function applyCategoryLayout(g) {
    const nodes = g.nodes();
    const n = nodes.length;
    if (n === 0) return;
    const catMap = {};
    nodes.forEach(id => {
      const cat = g.getNodeAttribute(id, "category") || "";
      if (!catMap[cat]) catMap[cat] = [];
      catMap[cat].push(id);
    });
    const cats = Object.keys(catMap).sort();
    const nCats = cats.length;
    const outerR = Math.max(4, Math.sqrt(n) * 0.4);
    cats.forEach((cat, ci) => {
      const angle = (2 * Math.PI * ci) / nCats - Math.PI / 2;
      const cx = outerR * Math.cos(angle);
      const cy = outerR * Math.sin(angle);
      const group = catMap[cat];
      const innerR = Math.max(0.5, Math.sqrt(group.length) * 0.4);
      group.forEach((id, i) => {
        const a = (2 * Math.PI * i) / group.length;
        g.setNodeAttribute(id, "x", cx + innerR * Math.cos(a));
        g.setNodeAttribute(id, "y", cy + innerR * Math.sin(a));
      });
    });
  }

  /**
   * Minimal Fruchterman-Reingold force layout.
   * Assigns x/y attributes directly on a graphology Graph instance.
   * Runs synchronously (animate: false equivalent). Suitable for ≤200 nodes.
   */
  function applyForceLayout(g, iterations) {
    iterations = iterations || 100;
    const nodes = g.nodes();
    const n = nodes.length;
    if (n === 0) return;

    // Initialize random positions in [-1, 1].
    nodes.forEach(id => {
      g.setNodeAttribute(id, "x", Math.random() * 2 - 1);
      g.setNodeAttribute(id, "y", Math.random() * 2 - 1);
    });

    if (n === 1) return; // single node: center it

    // Fruchterman-Reingold constants.
    const area = 4; // bounding box area
    const k = Math.sqrt(area / n); // ideal spring length
    const kSq = k * k;

    for (let iter = 0; iter < iterations; iter++) {
      const temp = 1.0 * (1 - iter / iterations); // cooling factor

      // Build displacement accumulators.
      const dx = new Float64Array(n);
      const dy = new Float64Array(n);

      // Repulsion: all pairs.
      for (let i = 0; i < n; i++) {
        for (let j = i + 1; j < n; j++) {
          const xi = g.getNodeAttribute(nodes[i], "x");
          const yi = g.getNodeAttribute(nodes[i], "y");
          const xj = g.getNodeAttribute(nodes[j], "x");
          const yj = g.getNodeAttribute(nodes[j], "y");
          const ddx = xi - xj;
          const ddy = yi - yj;
          const dist = Math.sqrt(ddx * ddx + ddy * ddy) || 0.01;
          const force = kSq / dist;
          const fx = (ddx / dist) * force;
          const fy = (ddy / dist) * force;
          dx[i] += fx; dy[i] += fy;
          dx[j] -= fx; dy[j] -= fy;
        }
      }

      // Attraction: edges.
      g.forEachEdge((edge, attrs, source, target) => {
        const si = nodes.indexOf(source);
        const ti = nodes.indexOf(target);
        if (si < 0 || ti < 0) return;
        const xi = g.getNodeAttribute(source, "x");
        const yi = g.getNodeAttribute(source, "y");
        const xj = g.getNodeAttribute(target, "x");
        const yj = g.getNodeAttribute(target, "y");
        const ddx = xi - xj;
        const ddy = yi - yj;
        const dist = Math.sqrt(ddx * ddx + ddy * ddy) || 0.01;
        const force = (dist * dist) / k;
        const fx = (ddx / dist) * force;
        const fy = (ddy / dist) * force;
        dx[si] -= fx; dy[si] -= fy;
        dx[ti] += fx; dy[ti] += fy;
      });

      // Apply displacement with temperature clamping.
      for (let i = 0; i < n; i++) {
        const dispLen = Math.sqrt(dx[i] * dx[i] + dy[i] * dy[i]) || 0.01;
        const clamped = Math.min(dispLen, temp);
        const newX = g.getNodeAttribute(nodes[i], "x") + (dx[i] / dispLen) * clamped;
        const newY = g.getNodeAttribute(nodes[i], "y") + (dy[i] / dispLen) * clamped;
        g.setNodeAttribute(nodes[i], "x", newX);
        g.setNodeAttribute(nodes[i], "y", newY);
      }
    }

    // Normalize to [0, 1] bounding box.
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    nodes.forEach(id => {
      const x = g.getNodeAttribute(id, "x");
      const y = g.getNodeAttribute(id, "y");
      if (x < minX) minX = x; if (x > maxX) maxX = x;
      if (y < minY) minY = y; if (y > maxY) maxY = y;
    });
    const rangeX = maxX - minX || 1;
    const rangeY = maxY - minY || 1;
    nodes.forEach(id => {
      g.setNodeAttribute(id, "x", (g.getNodeAttribute(id, "x") - minX) / rangeX);
      g.setNodeAttribute(id, "y", (g.getNodeAttribute(id, "y") - minY) / rangeY);
    });
  }

  /**
   * Read theme-driven colors from CSS vars.
   * Returns {labelColor, edgeSimilarityColor, edgeExplicitColor, bgColor}.
   */
  function themeColors() {
    const s = getComputedStyle(document.documentElement);
    return {
      labelColor:          s.getPropertyValue("--text-0").trim()  || "#c0caf5",
      edgeSimilarityColor: s.getPropertyValue("--accent-warm").trim() || "#e0af68",
      edgeExplicitColor:   s.getPropertyValue("--accent").trim()  || "#7aa2f7",
      bgColor:             s.getPropertyValue("--bg-0").trim()    || "#1a1b26",
    };
  }

  async function renderGraph(data) {
    graphData = data;
    const container = document.getElementById("cy");
    if (!container) return;

    // Destroy previous sigma instance.
    if (sigmaInstance) {
      sigmaInstance.kill();
      sigmaInstance = null;
    }

    if (!data || !data.nodes || data.nodes.length === 0) {
      while (container.firstChild) container.removeChild(container.firstChild);
      const msg = document.createElement("div");
      msg.style.cssText = "padding:20px;color:var(--text-muted);font-size:var(--text-sm)";
      msg.textContent = "No graph data.";
      container.appendChild(msg);
      return;
    }

    // Bail if sigma or graphology not loaded.
    if (typeof Sigma === "undefined" || typeof graphology === "undefined") return;

    // Only render nodes that participate in at least one edge.
    // Falls back to all nodes when there are no edges.
    const connectedPaths = new Set();
    data.edges.forEach(e => {
      const src = e.src && e.src.path ? e.src.path : e.src;
      const dst = e.dst && e.dst.path ? e.dst.path : e.dst;
      connectedPaths.add(src);
      connectedPaths.add(dst);
    });
    const nodesToRender = connectedPaths.size > 0
      ? data.nodes.filter(n => connectedPaths.has(n.path))
      : data.nodes;

    // Show loading indicator, then yield so the browser paints it before
    // blocking on graph construction.
    container.setAttribute("data-loading", "true");
    await new Promise(resolve => requestAnimationFrame(() => setTimeout(resolve, 0)));

    // Build graphology graph.
    const g = new graphology.Graph({ type: "undirected", multi: false });

    nodesToRender.forEach(n => {
      if (!g.hasNode(n.path)) {
        g.addNode(n.path, {
          label: n.name || n.path,
          color: categoryColor(n.category || ""),
          size: 5,
          category: n.category || "",
          // x/y set by layout below
        });
      }
    });

    // Deduplicate edges (similarity is symmetric).
    const seen = new Set();
    data.edges.forEach(e => {
      const src = e.src && e.src.path ? e.src.path : e.src;
      const dst = e.dst && e.dst.path ? e.dst.path : e.dst;
      const key = [src, dst].sort().join("|");
      if (seen.has(key)) return;
      seen.add(key);
      if (!g.hasNode(src) || !g.hasNode(dst)) return;
      try {
        g.addEdge(src, dst, {
          kind: e.kind,
          score: e.score,
          color: e.kind === "explicit" ? themeColors().edgeExplicitColor : themeColors().edgeSimilarityColor,
          size: e.kind === "explicit" ? 2 : 3,
        });
      } catch (_) { /* duplicate edge guard */ }
    });

    // Category radial layout for large graphs; force layout for small ones.
    if (nodesToRender.length > 50) {
      applyCategoryLayout(g);
    } else {
      applyForceLayout(g, 100);
    }

    const colors = themeColors();

    // Instantiate sigma renderer.
    sigmaInstance = new Sigma(g, container, {
      renderLabels: true,
      labelColor: { color: colors.labelColor },
      labelSize: 10,
      labelWeight: "normal",
      defaultNodeColor: "#7aa2f7",
      defaultEdgeColor: colors.edgeSimilarityColor,
      allowInvalidContainer: true,
    });

    container.removeAttribute("data-loading");

    // Expose sigma instance for Playwright tests.
    window.__grugSigma = sigmaInstance;

    // Click node → navigate to that memory (preserves Plan 1 behavior).
    // router is not yet extracted — use window.router to avoid forward-reference.
    sigmaInstance.on("clickNode", evt => {
      const nodeId = evt.node;
      const node = data.nodes.find(n => n.path === nodeId);
      if (node && window.router) {
        window.router.navigate({ memoryPath: node.path, memoryCategory: node.category });
      }
    });
  }

  /**
   * Update sigma colors from current CSS vars after theme change.
   * Called by theme.toggle() to keep graph in sync with light/dark/system mode.
   */
  function updateTheme() {
    if (!sigmaInstance) return;
    const colors = themeColors();
    sigmaInstance.setSetting("labelColor", { color: colors.labelColor });
    sigmaInstance.refresh();
  }

  /**
   * Render a local N-hop neighborhood. Same shape as `render`, but the
   * node matching `focusPath` (if given) is enlarged + accent-colored so
   * users can locate the focused memory in the layout.
   */
  async function renderLocal(data, opts) {
    opts = opts || {};
    await renderGraph(data);
    if (!sigmaInstance) return;
    const focusPath = opts.focusPath;
    if (!focusPath) return;
    try {
      const g = sigmaInstance.getGraph();
      if (g.hasNode(focusPath)) {
        const accent = themeColors().edgeExplicitColor;
        g.setNodeAttribute(focusPath, "size", 10);
        g.setNodeAttribute(focusPath, "color", accent);
        sigmaInstance.refresh();
      }
    } catch (_) { /* sigma version mismatch — ignore */ }
  }

  return { render: renderGraph, renderLocal, updateTheme,
           refresh: () => sigmaInstance && sigmaInstance.refresh() };
})();
