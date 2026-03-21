// ─── State ────────────────────────────────────────────────────────────────

let currentView = 'dashboard';
let browseState = { page: 1, perPage: 50, filters: {} };
let registriesState = { page: 1, perPage: 50, filters: {} };
let personsState = { page: 1, perPage: 50, filters: {} };
let viewerState = { zoom: 1, pages: [], currentIndex: 0, manifestId: null };

// ─── Navigation ──────────────────────────────────────────────────────────

function navigateTo(view, params) {
  document.querySelectorAll('#content > div').forEach(d => d.style.display = 'none');
  document.querySelectorAll('.nav-link').forEach(a => a.classList.remove('active'));

  const navLink = document.querySelector(`.nav-link[data-view="${view}"]`);
  if (navLink) navLink.classList.add('active');

  currentView = view;
  const el = document.getElementById(`view-${view}`);
  if (el) el.style.display = '';

  switch (view) {
    case 'dashboard': loadDashboard(); break;
    case 'browse': loadBrowse(); break;
    case 'registries': loadRegistries(); break;
    case 'manifest': loadManifest(params.id); break;
    case 'viewer': loadViewer(params.manifestId, params.pageIndex); break;
    case 'persons': loadPersons(); break;
    case 'search': loadSearch(); break;
  }
}

// ─── Safe DOM Helpers ────────────────────────────────────────────────────
// All data originates from the local SQLite database (user's own data).
// We still escape all dynamic values to prevent any accidental injection.

function esc(str) {
  if (str == null) return '';
  const d = document.createElement('div');
  d.textContent = String(str);
  return d.innerHTML;
}

function formatNumber(n) {
  return (n || 0).toLocaleString('it-IT');
}

function setText(el, text) {
  el.textContent = text;
}

function setContent(el, trustedHtml) {
  // trustedHtml is built from esc()-escaped values only
  el.innerHTML = trustedHtml; // safe: all dynamic values are escaped via esc()
}

// ─── API Helpers ─────────────────────────────────────────────────────────

async function api(path) {
  const res = await fetch(`/api/v1${path}`);
  if (!res.ok) throw new Error(`API error: ${res.status}`);
  return res.json();
}

// ─── Dashboard ───────────────────────────────────────────────────────────

async function loadDashboard() {
  const el = document.getElementById('view-dashboard');
  el.textContent = '';
  el.setAttribute('aria-busy', 'true');

  try {
    const stats = await api('/stats');
    const b = stats.base;

    // Build dashboard using escaped values only
    const html = `
      <h2>Dashboard</h2>
      <div class="stats-grid">
        <div class="stat-card">
          <span class="stat-value">${esc(formatNumber(b.manifests))}</span>
          <span class="stat-label">Registri</span>
        </div>
        <div class="stat-card">
          <span class="stat-value">${esc(formatNumber(b.complete))}</span>
          <span class="stat-label">Immagini scaricate</span>
        </div>
        <div class="stat-card">
          <span class="stat-value">${esc(formatNumber(stats.archives))}</span>
          <span class="stat-label">Archivi</span>
        </div>
        <div class="stat-card">
          <span class="stat-value">${esc(formatNumber(stats.localities))}</span>
          <span class="stat-label">Localit&agrave;</span>
        </div>
        <div class="stat-card">
          <span class="stat-value">${esc(formatNumber(stats.persons))}</span>
          <span class="stat-label">Persone</span>
        </div>
        <div class="stat-card">
          <span class="stat-value">${esc(formatNumber(stats.ocr_results))}</span>
          <span class="stat-label">Risultati OCR</span>
        </div>
        <div class="stat-card">
          <span class="stat-value">${esc(formatNumber(b.tags))}</span>
          <span class="stat-label">Tag</span>
        </div>
        <div class="stat-card">
          <span class="stat-value">${esc(formatNumber(b.pending))}</span>
          <span class="stat-label">In attesa</span>
        </div>
      </div>

      <div class="grid">
        <article>
          <h3>Download</h3>
          <p>
            <strong>${esc(formatNumber(b.total_downloads))}</strong> totali &mdash;
            ${esc(formatNumber(b.complete))} completati,
            ${esc(formatNumber(b.failed))} falliti,
            ${esc(formatNumber(b.pending))} in attesa
          </p>
          ${b.total_downloads > 0 ? `<progress value="${Number(b.complete)}" max="${Number(b.total_downloads)}"></progress>` : ''}
        </article>
        <article>
          <h3>Ricerca</h3>
          <p>
            <strong>${esc(formatNumber(stats.search_queries))}</strong> ricerche effettuate,
            <strong>${esc(formatNumber(stats.registry_results))}</strong> risultati trovati
          </p>
        </article>
        <article>
          <h3>Catalogo Registri</h3>
          <p><strong>${esc(formatNumber(stats.registries))}</strong> registri catalogati</p>
        </article>
      </div>
    `;

    el.removeAttribute('aria-busy');
    setContent(el, html);
  } catch (e) {
    el.removeAttribute('aria-busy');
    el.textContent = `Errore: ${e.message}`;
  }
}

// ─── Browse ──────────────────────────────────────────────────────────────

async function loadBrowse() {
  const el = document.getElementById('view-browse');

  // Only render filter UI once
  if (!el.dataset.initialized) {
    el.dataset.initialized = 'true';
    setContent(el, `
      <h2>Sfoglia Registri</h2>
      <div class="filters" id="browse-filters">
        <div class="filter-group">
          <label>Tipo documento</label>
          <select id="filter-doc-type"><option value="">Tutti</option></select>
        </div>
        <div class="filter-group">
          <label>Anno</label>
          <input type="text" id="filter-year" placeholder="es. 1810">
        </div>
        <div class="filter-group">
          <label>Archivio</label>
          <select id="filter-archive"><option value="">Tutti</option></select>
        </div>
        <div class="filter-group">
          <label>Localit&agrave;</label>
          <input type="text" id="filter-locality" placeholder="es. Camposano">
        </div>
        <div class="filter-group" style="align-self:end">
          <button onclick="applyBrowseFilters()" class="outline">Filtra</button>
        </div>
      </div>
      <div id="browse-results"></div>
    `);

    // Populate filter dropdowns
    const [docTypes, archives] = await Promise.all([
      api('/facets/doc_types'),
      api('/archives'),
    ]);

    const docSel = document.getElementById('filter-doc-type');
    docTypes.forEach(dt => {
      const opt = document.createElement('option');
      opt.value = dt;
      opt.textContent = dt;
      docSel.appendChild(opt);
    });

    const archSel = document.getElementById('filter-archive');
    archives.forEach(a => {
      const opt = document.createElement('option');
      opt.value = a.name;
      opt.textContent = a.name;
      archSel.appendChild(opt);
    });

    // Enter key triggers filter
    document.getElementById('filter-year').addEventListener('keydown', e => {
      if (e.key === 'Enter') applyBrowseFilters();
    });
    document.getElementById('filter-locality').addEventListener('keydown', e => {
      if (e.key === 'Enter') applyBrowseFilters();
    });
  }

  applyBrowseFilters();
}

async function applyBrowseFilters() {
  browseState.filters = {
    doc_type: document.getElementById('filter-doc-type')?.value || '',
    year: document.getElementById('filter-year')?.value || '',
    archive: document.getElementById('filter-archive')?.value || '',
    locality: document.getElementById('filter-locality')?.value || '',
  };
  browseState.page = 1;
  await fetchBrowseResults();
}

async function fetchBrowseResults() {
  const el = document.getElementById('browse-results');
  el.textContent = '';
  el.setAttribute('aria-busy', 'true');

  const f = browseState.filters;
  let qs = `?page=${browseState.page}&per_page=${browseState.perPage}`;
  if (f.doc_type) qs += `&doc_type=${encodeURIComponent(f.doc_type)}`;
  if (f.year) qs += `&year=${encodeURIComponent(f.year)}`;
  if (f.archive) qs += `&archive=${encodeURIComponent(f.archive)}`;
  if (f.locality) qs += `&locality=${encodeURIComponent(f.locality)}`;

  try {
    const data = await api(`/manifests${qs}`);
    el.removeAttribute('aria-busy');

    if (data.data.length === 0) {
      el.textContent = 'Nessun registro trovato.';
      return;
    }

    const totalPages = Math.ceil(data.total / data.per_page);

    // Build table rows with escaped values
    const rows = data.data.map(m =>
      `<tr class="clickable-row" data-manifest-id="${esc(m.id)}">
        <td>${esc(m.title) || esc(m.id)}</td>
        <td>${esc(m.doc_type) || '-'}</td>
        <td>${esc(m.year) || '-'}</td>
        <td>${esc(m.archive_name) || '-'}</td>
        <td>${m.total_canvases != null ? esc(String(m.total_canvases)) : '-'}</td>
      </tr>`
    ).join('');

    let paginationHtml = '';
    if (totalPages > 1) {
      paginationHtml = `<div class="pagination">
        <button id="browse-prev" ${browseState.page <= 1 ? 'disabled' : ''}>&laquo; Prec</button>
        <span class="page-info">Pagina ${esc(String(browseState.page))} di ${esc(String(totalPages))}</span>
        <button id="browse-next" ${browseState.page >= totalPages ? 'disabled' : ''}>Succ &raquo;</button>
      </div>`;
    }

    setContent(el, `
      <p>${esc(formatNumber(data.total))} registri trovati</p>
      <table role="grid"><thead><tr>
        <th>Titolo</th><th>Tipo</th><th>Anno</th><th>Archivio</th><th>Pagine</th>
      </tr></thead><tbody>${rows}</tbody></table>
      ${paginationHtml}
    `);

    // Attach click handlers via event delegation
    el.querySelectorAll('.clickable-row').forEach(row => {
      row.addEventListener('click', () => {
        navigateTo('manifest', { id: row.dataset.manifestId });
      });
    });

    const prevBtn = document.getElementById('browse-prev');
    const nextBtn = document.getElementById('browse-next');
    if (prevBtn) prevBtn.addEventListener('click', () => { browseState.page--; fetchBrowseResults(); });
    if (nextBtn) nextBtn.addEventListener('click', () => { browseState.page++; fetchBrowseResults(); });

  } catch (e) {
    el.removeAttribute('aria-busy');
    el.textContent = `Errore: ${e.message}`;
  }
}

// ─── Registries Catalog ──────────────────────────────────────────────────

async function loadRegistries() {
  const el = document.getElementById('view-registries');

  if (!el.dataset.initialized) {
    el.dataset.initialized = 'true';
    setContent(el, `
      <h2>Catalogo Registri</h2>
      <div class="filters" id="reg-filters">
        <div class="filter-group">
          <label>Tipo documento</label>
          <select id="reg-filter-doc-type"><option value="">Tutti</option></select>
        </div>
        <div class="filter-group">
          <label>Anno</label>
          <input type="text" id="reg-filter-year" placeholder="es. 1810">
        </div>
        <div class="filter-group">
          <label>Archivio</label>
          <select id="reg-filter-archive"><option value="">Tutti</option></select>
        </div>
        <div class="filter-group">
          <label>Localit&agrave;</label>
          <input type="text" id="reg-filter-locality" placeholder="es. Camposano">
        </div>
        <div class="filter-group">
          <label><input type="checkbox" id="reg-filter-has-images"> Solo con immagini</label>
        </div>
        <div class="filter-group" style="align-self:end">
          <button onclick="applyRegistryFilters()" class="outline">Filtra</button>
        </div>
      </div>
      <div id="reg-results"></div>
    `);

    try {
      const facets = await api('/registries/facets');

      const docSel = document.getElementById('reg-filter-doc-type');
      facets.doc_types.forEach(dt => {
        const opt = document.createElement('option');
        opt.value = dt;
        opt.textContent = dt;
        docSel.appendChild(opt);
      });

      const archSel = document.getElementById('reg-filter-archive');
      facets.archives.forEach(a => {
        const opt = document.createElement('option');
        opt.value = a;
        opt.textContent = a;
        archSel.appendChild(opt);
      });
    } catch (e) {
      // Facets may fail if no data yet
    }

    document.getElementById('reg-filter-year').addEventListener('keydown', e => {
      if (e.key === 'Enter') applyRegistryFilters();
    });
    document.getElementById('reg-filter-locality').addEventListener('keydown', e => {
      if (e.key === 'Enter') applyRegistryFilters();
    });
  }

  applyRegistryFilters();
}

async function applyRegistryFilters() {
  registriesState.filters = {
    doc_type: document.getElementById('reg-filter-doc-type')?.value || '',
    year: document.getElementById('reg-filter-year')?.value || '',
    archive: document.getElementById('reg-filter-archive')?.value || '',
    locality: document.getElementById('reg-filter-locality')?.value || '',
    has_images: document.getElementById('reg-filter-has-images')?.checked || false,
  };
  registriesState.page = 1;
  await fetchRegistryResults();
}

async function fetchRegistryResults() {
  const el = document.getElementById('reg-results');
  el.textContent = '';
  el.setAttribute('aria-busy', 'true');

  const f = registriesState.filters;
  let qs = `?page=${registriesState.page}&per_page=${registriesState.perPage}`;
  if (f.doc_type) qs += `&doc_type=${encodeURIComponent(f.doc_type)}`;
  if (f.year) qs += `&year=${encodeURIComponent(f.year)}`;
  if (f.archive) qs += `&archive=${encodeURIComponent(f.archive)}`;
  if (f.locality) qs += `&locality=${encodeURIComponent(f.locality)}`;
  if (f.has_images) qs += `&has_images=true`;

  try {
    const data = await api(`/registries${qs}`);
    el.removeAttribute('aria-busy');

    if (data.data.length === 0) {
      el.textContent = 'Nessun registro trovato.';
      return;
    }

    const totalPages = Math.ceil(data.total / data.per_page);

    const rows = data.data.map(r => {
      const arkShort = r.ark_url.split('/').pop() || r.ark_url;
      return `<tr>
        <td>${esc(r.year) || '-'}</td>
        <td>${esc(r.doc_type) || '-'}</td>
        <td>${esc(r.locality_name) || '-'}</td>
        <td>${esc(r.province) || '-'}</td>
        <td>${esc(r.archive_name) || '-'}</td>
        <td>${esc(r.signature) || '-'}</td>
        <td><a href="${esc(r.ark_url)}" target="_blank">${esc(arkShort)}</a></td>
        <td>${r.has_images ? '\u2713' : '-'}</td>
      </tr>`;
    }).join('');

    let paginationHtml = '';
    if (totalPages > 1) {
      paginationHtml = `<div class="pagination">
        <button id="reg-prev" ${registriesState.page <= 1 ? 'disabled' : ''}>&laquo; Prec</button>
        <span class="page-info">Pagina ${esc(String(registriesState.page))} di ${esc(String(totalPages))}</span>
        <button id="reg-next" ${registriesState.page >= totalPages ? 'disabled' : ''}>Succ &raquo;</button>
      </div>`;
    }

    setContent(el, `
      <p>${esc(formatNumber(data.total))} registri trovati</p>
      <table role="grid"><thead><tr>
        <th>Anno</th><th>Tipo</th><th>Localit\u00e0</th><th>Provincia</th><th>Archivio</th><th>Segnatura</th><th>ARK</th><th>Immagini</th>
      </tr></thead><tbody>${rows}</tbody></table>
      ${paginationHtml}
    `);

    const prevBtn = document.getElementById('reg-prev');
    const nextBtn = document.getElementById('reg-next');
    if (prevBtn) prevBtn.addEventListener('click', () => { registriesState.page--; fetchRegistryResults(); });
    if (nextBtn) nextBtn.addEventListener('click', () => { registriesState.page++; fetchRegistryResults(); });

  } catch (e) {
    el.removeAttribute('aria-busy');
    el.textContent = `Errore: ${e.message}`;
  }
}

// ─── Manifest Detail ─────────────────────────────────────────────────────

async function loadManifest(id) {
  const el = document.getElementById('view-manifest');
  el.textContent = '';
  el.setAttribute('aria-busy', 'true');

  try {
    const [manifest, pages, metadata] = await Promise.all([
      api(`/manifests/${encodeURIComponent(id)}`),
      api(`/manifests/${encodeURIComponent(id)}/pages`),
      api(`/manifests/${encodeURIComponent(id)}/metadata`),
    ]);

    viewerState.pages = pages;
    viewerState.manifestId = id;

    el.removeAttribute('aria-busy');

    // Build metadata rows
    let metaHtml = '';
    if (metadata.length > 0) {
      const metaRows = metadata.map(m =>
        `<tr><th>${esc(m.label)}</th><td>${esc(m.value)}</td></tr>`
      ).join('');
      metaHtml = `<details><summary>Metadati IIIF (${esc(String(metadata.length))})</summary>
        <table class="metadata-table"><tbody>${metaRows}</tbody></table></details>`;
    }

    // Build manifest meta line
    const metaParts = [];
    if (manifest.doc_type) metaParts.push(`<span><strong>Tipo:</strong> ${esc(manifest.doc_type)}</span>`);
    if (manifest.year) metaParts.push(`<span><strong>Anno:</strong> ${esc(manifest.year)}</span>`);
    if (manifest.archive_name) metaParts.push(`<span><strong>Archivio:</strong> ${esc(manifest.archive_name)}</span>`);
    if (manifest.archival_context) metaParts.push(`<span><strong>Contesto:</strong> ${esc(manifest.archival_context)}</span>`);
    if (manifest.signature) metaParts.push(`<span><strong>Segnatura:</strong> ${esc(manifest.signature)}</span>`);
    if (manifest.date_from || manifest.date_to) metaParts.push(`<span><strong>Date:</strong> ${esc(manifest.date_from)} &mdash; ${esc(manifest.date_to)}</span>`);

    // Build page thumbnails
    const completedPages = pages.filter(p => p.status === 'complete' && p.local_path);
    let thumbsHtml = '';
    if (completedPages.length > 0) {
      const thumbCards = pages.map((p, idx) => {
        if (p.status !== 'complete' || !p.local_path) return '';
        const imgSrc = `/images/${encodeURI(p.local_path.replace(/^\.?\/?antenati\//, ''))}`;
        return `<div class="thumb-card" data-page-index="${idx}">
          <img src="${esc(imgSrc)}" alt="${esc(p.canvas_label)}" loading="lazy">
          <div class="thumb-label">${esc(p.canvas_label) || `Pag. ${p.canvas_index + 1}`}</div>
        </div>`;
      }).join('');
      thumbsHtml = `<div class="thumb-grid">${thumbCards}</div>`;
    } else {
      thumbsHtml = '<div class="empty-state">Nessuna immagine scaricata per questo registro.</div>';
    }

    setContent(el, `
      <div class="manifest-header">
        <a href="#" id="manifest-back">&laquo; Torna alla lista</a>
        <h2>${esc(manifest.title) || esc(manifest.id)}</h2>
        <div class="manifest-meta">${metaParts.join('')}</div>
      </div>
      ${metaHtml}
      <h3>${esc(String(completedPages.length))} pagine scaricate di ${esc(String(pages.length))}</h3>
      ${thumbsHtml}
    `);

    // Attach event handlers
    document.getElementById('manifest-back')?.addEventListener('click', (e) => {
      e.preventDefault();
      navigateTo('browse');
    });

    el.querySelectorAll('.thumb-card').forEach(card => {
      card.addEventListener('click', () => {
        const idx = parseInt(card.dataset.pageIndex, 10);
        navigateTo('viewer', { manifestId: viewerState.manifestId, pageIndex: idx });
      });
    });

  } catch (e) {
    el.removeAttribute('aria-busy');
    el.textContent = `Errore: ${e.message}`;
  }
}

// ─── Image Viewer ────────────────────────────────────────────────────────

async function loadViewer(manifestId, pageIndex) {
  const el = document.getElementById('view-viewer');
  viewerState.zoom = 1;
  viewerState.currentIndex = pageIndex;

  // Ensure pages are loaded
  if (viewerState.manifestId !== manifestId || viewerState.pages.length === 0) {
    viewerState.manifestId = manifestId;
    viewerState.pages = await api(`/manifests/${encodeURIComponent(manifestId)}/pages`);
  }

  renderViewer(el);
}

async function renderViewer(el) {
  const page = viewerState.pages[viewerState.currentIndex];
  if (!page) {
    el.textContent = 'Pagina non trovata.';
    return;
  }

  const imgSrc = page.local_path ? `/images/${encodeURI(page.local_path.replace(/^\.?\/?antenati\//, ''))}` : '';
  const label = page.canvas_label || `Pagina ${page.canvas_index + 1}`;
  const total = viewerState.pages.length;
  const idx = viewerState.currentIndex;

  let sizeInfo = '';
  if (page.width && page.height) {
    sizeInfo = `<p style="font-size:0.8rem;color:var(--pico-muted-color)">${esc(String(page.width))} &times; ${esc(String(page.height))} px</p>`;
  }

  setContent(el, `
    <div class="viewer-controls">
      <a href="#" id="viewer-back">&laquo; Torna al registro</a>
      <div class="viewer-nav">
        <button id="viewer-prev" ${idx <= 0 ? 'disabled' : ''}>&laquo; Prec</button>
        <span class="page-info">${esc(String(idx + 1))} / ${esc(String(total))}</span>
        <button id="viewer-next" ${idx >= total - 1 ? 'disabled' : ''}>Succ &raquo;</button>
      </div>
      <div class="viewer-zoom">
        <button id="zoom-out">-</button>
        <span id="zoom-level">${Math.round(viewerState.zoom * 100)}%</span>
        <button id="zoom-in">+</button>
        <button id="zoom-reset">Reset</button>
      </div>
    </div>
    <div class="viewer-container">
      <div class="viewer-image-wrap" id="viewer-img-wrap">
        ${imgSrc ? `<img id="viewer-img" src="${esc(imgSrc)}" alt="${esc(label)}" style="transform: scale(${viewerState.zoom})">` : '<div class="empty-state">Immagine non disponibile</div>'}
      </div>
      <div class="viewer-sidebar">
        <h4>${esc(label)}</h4>
        ${sizeInfo}
        <div id="viewer-ocr"><div aria-busy="true">OCR...</div></div>
        <div id="viewer-tags"></div>
      </div>
    </div>
  `);

  // Attach event handlers
  document.getElementById('viewer-back')?.addEventListener('click', (e) => {
    e.preventDefault();
    navigateTo('manifest', { id: viewerState.manifestId });
  });
  document.getElementById('viewer-prev')?.addEventListener('click', () => viewerNav(-1));
  document.getElementById('viewer-next')?.addEventListener('click', () => viewerNav(1));
  document.getElementById('zoom-out')?.addEventListener('click', () => viewerZoom(-0.25));
  document.getElementById('zoom-in')?.addEventListener('click', () => viewerZoom(0.25));
  document.getElementById('zoom-reset')?.addEventListener('click', () => viewerZoom(0, 1));

  // Keyboard navigation
  document.onkeydown = (e) => {
    if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA') return;
    if (e.key === 'ArrowLeft') viewerNav(-1);
    else if (e.key === 'ArrowRight') viewerNav(1);
    else if (e.key === '+' || e.key === '=') viewerZoom(0.25);
    else if (e.key === '-') viewerZoom(-0.25);
    else if (e.key === '0') viewerZoom(0, 1);
  };

  // Load OCR and tags asynchronously
  loadViewerSidebar(page.id);
}

async function loadViewerSidebar(downloadId) {
  try {
    const [ocrResults, tags] = await Promise.all([
      api(`/downloads/${downloadId}/ocr`),
      api(`/downloads/${downloadId}/tags`),
    ]);

    const ocrEl = document.getElementById('viewer-ocr');
    if (ocrEl) {
      if (ocrResults.length > 0) {
        const text = ocrResults.map(o => o.raw_text || '').join('\n---\n');
        const h5 = document.createElement('h5');
        h5.textContent = 'Testo OCR';
        const pre = document.createElement('div');
        pre.className = 'ocr-text';
        pre.textContent = text;
        ocrEl.textContent = '';
        ocrEl.appendChild(h5);
        ocrEl.appendChild(pre);
      } else {
        ocrEl.textContent = 'Nessun risultato OCR';
        ocrEl.style.fontSize = '0.85rem';
        ocrEl.style.color = 'var(--pico-muted-color)';
      }
    }

    const tagsEl = document.getElementById('viewer-tags');
    if (tagsEl && tags.length > 0) {
      const h5 = document.createElement('h5');
      h5.textContent = 'Tag';
      const list = document.createElement('div');
      list.className = 'tag-list';
      tags.forEach(t => {
        const span = document.createElement('span');
        span.className = 'tag';
        span.title = `${t.tag_type}: ${t.value}`;
        span.textContent = `${t.tag_type}: ${t.value}`;
        list.appendChild(span);
      });
      tagsEl.appendChild(h5);
      tagsEl.appendChild(list);
    }
  } catch (_) {
    // Sidebar data is optional, ignore errors
  }
}

function viewerNav(delta) {
  const newIdx = viewerState.currentIndex + delta;
  if (newIdx < 0 || newIdx >= viewerState.pages.length) return;
  viewerState.currentIndex = newIdx;
  viewerState.zoom = 1;
  renderViewer(document.getElementById('view-viewer'));
}

function viewerZoom(delta, absolute) {
  if (absolute !== undefined) {
    viewerState.zoom = absolute;
  } else {
    viewerState.zoom = Math.max(0.25, Math.min(5, viewerState.zoom + delta));
  }
  const img = document.getElementById('viewer-img');
  if (img) img.style.transform = `scale(${viewerState.zoom})`;
  const label = document.getElementById('zoom-level');
  if (label) label.textContent = `${Math.round(viewerState.zoom * 100)}%`;
}

// ─── Persons ─────────────────────────────────────────────────────────────

async function loadPersons() {
  const el = document.getElementById('view-persons');

  if (!el.dataset.initialized) {
    el.dataset.initialized = 'true';
    setContent(el, `
      <h2>Ricerca Persone</h2>
      <div class="filters">
        <div class="filter-group">
          <label>Cognome</label>
          <input type="text" id="person-surname" placeholder="es. Rossi">
        </div>
        <div class="filter-group">
          <label>Nome</label>
          <input type="text" id="person-name" placeholder="es. Giovanni">
        </div>
        <div class="filter-group" style="align-self:end">
          <button id="person-search-btn" class="outline">Cerca</button>
        </div>
      </div>
      <div id="person-results"></div>
    `);

    document.getElementById('person-search-btn').addEventListener('click', applyPersonFilters);
    document.getElementById('person-surname').addEventListener('keydown', e => {
      if (e.key === 'Enter') applyPersonFilters();
    });
    document.getElementById('person-name').addEventListener('keydown', e => {
      if (e.key === 'Enter') applyPersonFilters();
    });
  }
}

async function applyPersonFilters() {
  personsState.filters = {
    surname: document.getElementById('person-surname')?.value || '',
    name: document.getElementById('person-name')?.value || '',
  };
  personsState.page = 1;
  await fetchPersonResults();
}

async function fetchPersonResults() {
  const el = document.getElementById('person-results');
  el.textContent = '';
  el.setAttribute('aria-busy', 'true');

  const f = personsState.filters;
  let qs = `?page=${personsState.page}&per_page=${personsState.perPage}`;
  if (f.surname) qs += `&surname=${encodeURIComponent(f.surname)}`;
  if (f.name) qs += `&name=${encodeURIComponent(f.name)}`;

  try {
    const data = await api(`/persons${qs}`);
    el.removeAttribute('aria-busy');

    if (data.data.length === 0) {
      el.textContent = 'Nessuna persona trovata.';
      return;
    }

    const totalPages = Math.ceil(data.total / data.per_page);

    const rows = data.data.map(p =>
      `<tr class="clickable-row" data-person-id="${esc(String(p.id))}" data-person-name="${esc(p.name)}">
        <td>${esc(p.surname) || '-'}</td>
        <td>${esc(p.given_name) || '-'}</td>
        <td>${esc(p.birth_info) || '-'}</td>
        <td>${esc(p.death_info) || '-'}</td>
      </tr>`
    ).join('');

    let paginationHtml = '';
    if (totalPages > 1) {
      paginationHtml = `<div class="pagination">
        <button id="persons-prev" ${personsState.page <= 1 ? 'disabled' : ''}>&laquo; Prec</button>
        <span class="page-info">Pagina ${esc(String(personsState.page))} di ${esc(String(totalPages))}</span>
        <button id="persons-next" ${personsState.page >= totalPages ? 'disabled' : ''}>Succ &raquo;</button>
      </div>`;
    }

    setContent(el, `
      <p>${esc(formatNumber(data.total))} persone trovate</p>
      <table role="grid"><thead><tr>
        <th>Cognome</th><th>Nome</th><th>Nascita</th><th>Morte</th>
      </tr></thead><tbody>${rows}</tbody></table>
      ${paginationHtml}
    `);

    // Attach click handlers
    el.querySelectorAll('.clickable-row').forEach(row => {
      row.addEventListener('click', () => {
        loadPersonDetail(parseInt(row.dataset.personId, 10), row.dataset.personName);
      });
    });

    const prevBtn = document.getElementById('persons-prev');
    const nextBtn = document.getElementById('persons-next');
    if (prevBtn) prevBtn.addEventListener('click', () => { personsState.page--; fetchPersonResults(); });
    if (nextBtn) nextBtn.addEventListener('click', () => { personsState.page++; fetchPersonResults(); });

  } catch (e) {
    el.removeAttribute('aria-busy');
    el.textContent = `Errore: ${e.message}`;
  }
}

async function loadPersonDetail(personId, name) {
  const el = document.getElementById('person-results');
  el.textContent = '';
  el.setAttribute('aria-busy', 'true');

  try {
    const records = await api(`/persons/${personId}`);
    el.removeAttribute('aria-busy');

    let html = `<a href="#" id="person-back">&laquo; Torna ai risultati</a>`;
    html += `<h3>${esc(name)}</h3>`;

    if (records.length === 0) {
      html += '<div class="empty-state">Nessun atto collegato.</div>';
    } else {
      const rows = records.map(r => {
        const hasManifest = r.manifest_id;
        return `<tr ${hasManifest ? `class="clickable-row" data-manifest-id="${esc(r.manifest_id)}"` : ''}>
          <td>${esc(r.record_type) || '-'}</td>
          <td>${esc(r.date) || '-'}</td>
          <td>${hasManifest ? 'Visualizza &rarr;' : (esc(r.ark_url) || '-')}</td>
        </tr>`;
      }).join('');

      html += `<table role="grid"><thead><tr>
        <th>Tipo atto</th><th>Data</th><th>Registro</th>
      </tr></thead><tbody>${rows}</tbody></table>`;
    }

    setContent(el, html);

    document.getElementById('person-back')?.addEventListener('click', (e) => {
      e.preventDefault();
      fetchPersonResults();
    });

    el.querySelectorAll('.clickable-row').forEach(row => {
      row.addEventListener('click', () => {
        navigateTo('manifest', { id: row.dataset.manifestId });
      });
    });

  } catch (e) {
    el.removeAttribute('aria-busy');
    el.textContent = `Errore: ${e.message}`;
  }
}

// ─── OCR Search ──────────────────────────────────────────────────────────

async function loadSearch() {
  const el = document.getElementById('view-search');

  if (!el.dataset.initialized) {
    el.dataset.initialized = 'true';
    setContent(el, `
      <h2>Ricerca Testo OCR</h2>
      <div class="filters">
        <div class="filter-group" style="flex:1">
          <input type="search" id="ocr-query" placeholder="Cerca nel testo dei documenti...">
        </div>
        <div class="filter-group" style="align-self:end">
          <button id="ocr-search-btn" class="outline">Cerca</button>
        </div>
      </div>
      <div id="ocr-results"></div>
    `);

    document.getElementById('ocr-search-btn').addEventListener('click', doOcrSearch);
    document.getElementById('ocr-query').addEventListener('keydown', e => {
      if (e.key === 'Enter') doOcrSearch();
    });
  }
}

async function doOcrSearch() {
  const query = document.getElementById('ocr-query')?.value?.trim();
  const el = document.getElementById('ocr-results');

  if (!query) {
    el.textContent = '';
    return;
  }

  el.textContent = '';
  el.setAttribute('aria-busy', 'true');

  try {
    const results = await api(`/search/ocr?q=${encodeURIComponent(query)}&limit=100`);
    el.removeAttribute('aria-busy');

    if (results.length === 0) {
      el.textContent = 'Nessun risultato trovato.';
      return;
    }

    const cards = results.map((r, i) => {
      // Convert >>> and <<< markers from FTS5 snippet to <mark> tags
      const snippet = esc(r.snippet).replace(/&gt;&gt;&gt;/g, '<mark>').replace(/&lt;&lt;&lt;/g, '</mark>');
      return `<div class="search-result" data-result-index="${i}" data-manifest-id="${esc(r.manifest_id)}" data-canvas-index="${r.canvas_index}">
        <div class="result-title">${esc(r.manifest_title) || esc(r.manifest_id)}</div>
        <div class="result-meta">${esc(r.canvas_label) || `Pagina ${r.canvas_index + 1}`} &mdash; Backend: ${esc(r.backend)}</div>
        <div class="result-snippet">${snippet}</div>
      </div>`;
    }).join('');

    setContent(el, `<p>${esc(String(results.length))} risultati</p>${cards}`);

    el.querySelectorAll('.search-result').forEach(card => {
      card.addEventListener('click', () => {
        navigateTo('viewer', {
          manifestId: card.dataset.manifestId,
          pageIndex: parseInt(card.dataset.canvasIndex, 10),
        });
      });
    });

  } catch (e) {
    el.removeAttribute('aria-busy');
    el.textContent = `Errore: ${e.message}`;
  }
}

// ─── Init ────────────────────────────────────────────────────────────────

navigateTo('dashboard');
