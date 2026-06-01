<script>
  import { onMount } from 'svelte';
  import {
    listFiles, uploadFile, downloadUrl, deleteFile, moveFile, syncDrive,
    search, chatWithFiles, fetchText, previewKind, formatSize,
    authStatus, deviceStart, devicePoll,
    listTrash, restoreTrash, fileHistory, restoreVersion, createShare, getUsage,
  } from './api.js';

  // ---- Core state ----
  let view = $state('drive'); // drive | trash | chat
  let cwd = $state(''); // current folder prefix
  let entries = $state([]);
  let status = $state('');
  let busy = $state(false);
  let dragging = $state(false);
  let viewMode = $state(localStorage.getItem('nimbus_view') || 'list');
  let theme = $state(localStorage.getItem('nimbus_theme') || 'dark');
  let sidebarOpen = $state(true);

  // ---- Search / chat ----
  let query = $state('');
  let results = $state(null);
  let chatQ = $state('');
  let chatAnswer = $state(null);
  let chatSources = $state([]);
  let chatting = $state(false);

  // ---- Trash ----
  let trash = $state([]);

  // ---- Overlays ----
  let preview = $state(null);
  let shareModal = $state(null); // { path, password, expires, result }
  let historyPanel = $state(null); // { path, commits }
  let paletteOpen = $state(false);
  let paletteQuery = $state('');
  let allFiles = $state([]);

  // ---- OAuth ----
  let oauthAvailable = $state(false);
  let device = $state(null);
  let connecting = $state(false);

  $effect(() => { document.documentElement.dataset.theme = theme; });

  const breadcrumb = $derived(cwd ? cwd.split('/') : []);

  let usage = $state(null); // { used, count, quota }

  async function refresh() {
    try { entries = await listFiles(cwd); } catch (e) { status = e.message; }
    loadUsage();
  }
  async function loadUsage() { try { usage = await getUsage(); } catch {} }

  onMount(async () => {
    await refresh();
    try { allFiles = await listFiles(); } catch {}
    try { oauthAvailable = (await authStatus()).oauth_available; } catch {}
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  function onKey(e) {
    if ((e.metaKey || e.ctrlKey) && e.key === 'k') { e.preventDefault(); togglePalette(); }
    else if (e.key === 'Escape') { paletteOpen = false; preview = null; shareModal = null; historyPanel = null; }
  }

  function setView(v) { view = v; status = ''; results = null; if (v === 'trash') loadTrash(); if (v === 'drive') refresh(); }
  function setViewMode(m) { viewMode = m; localStorage.setItem('nimbus_view', m); }
  function toggleTheme() { theme = theme === 'dark' ? 'light' : 'dark'; localStorage.setItem('nimbus_theme', theme); }

  function navigateInto(folder) { cwd = folder.path; refresh(); }
  function crumbTo(i) { cwd = breadcrumb.slice(0, i + 1).join('/'); refresh(); }
  function goRoot() { cwd = ''; refresh(); }

  async function handleFiles(fileList) {
    if (!fileList || fileList.length === 0) return;
    busy = true;
    try {
      for (const f of fileList) {
        status = `Uploading ${f.name}…`;
        const path = cwd ? `${cwd}/${f.name}` : f.name;
        await uploadFile(path, f);
      }
      status = `Uploaded ${fileList.length} file(s)`;
      await refresh();
    } catch (e) { status = e.message; } finally { busy = false; }
  }
  function onInputChange(e) { handleFiles(e.target.files); e.target.value = ''; }
  function onDrop(e) { e.preventDefault(); dragging = false; handleFiles(e.dataTransfer.files); }

  async function onSync() {
    busy = true; status = 'Syncing from GitHub…';
    try { await syncDrive(); await refresh(); allFiles = await listFiles(); status = 'Synced'; }
    catch (e) { status = e.message; } finally { busy = false; }
  }

  async function onSearch() {
    if (!query.trim()) { results = null; return; }
    busy = true;
    try {
      const hits = await search(query.trim());
      results = hits === null ? (status = 'Search disabled (set NIMBUS_AI_PROVIDER).', null) : hits;
    } catch (e) { status = e.message; } finally { busy = false; }
  }

  async function onChat() {
    if (!chatQ.trim()) return;
    chatting = true; chatAnswer = null; chatSources = [];
    try {
      const res = await chatWithFiles(chatQ.trim());
      if (res === null) status = 'Chat disabled (set NIMBUS_AI_PROVIDER).';
      else { chatAnswer = res.answer; chatSources = res.sources || []; }
    } catch (e) { status = e.message; } finally { chatting = false; }
  }

  async function openPreview(path) {
    const kind = previewKind(path);
    preview = { path, kind, text: null };
    if (kind === 'text') {
      try { preview = { path, kind, text: await fetchText(path) }; } catch (e) { status = e.message; }
    }
  }

  function basename(p) { return p.split('/').pop(); }

  async function renameEntry(entry) {
    const name = prompt('Rename to:', basename(entry.path));
    if (!name || name === basename(entry.path)) return;
    const newPath = cwd ? `${cwd}/${name}` : name;
    busy = true;
    try { await moveFile(entry.path, newPath); await refresh(); status = 'Renamed'; }
    catch (e) { status = e.message; } finally { busy = false; }
  }

  async function deleteEntry(entry) {
    if (!confirm(`Move "${basename(entry.path)}" to Trash?`)) return;
    busy = true;
    try { await deleteFile(entry.path); await refresh(); status = 'Moved to Trash'; }
    catch (e) { status = e.message; } finally { busy = false; }
  }

  function openShare(entry) { shareModal = { path: entry.path, password: '', expires: '', result: null }; }
  async function doShare() {
    busy = true;
    try {
      const expires = shareModal.expires ? Number(shareModal.expires) * 3600 : null;
      const res = await createShare(shareModal.path, shareModal.password || null, expires);
      shareModal = { ...shareModal, result: `${location.origin}${res.url}` };
    } catch (e) { status = e.message; } finally { busy = false; }
  }
  function copyShare() { navigator.clipboard?.writeText(shareModal.result); status = 'Link copied'; }

  async function openHistory(entry) {
    try { historyPanel = { path: entry.path, commits: await fileHistory(entry.path) }; }
    catch (e) { status = e.message; }
  }
  async function restoreCommit(commit) {
    busy = true;
    try { await restoreVersion(historyPanel.path, commit); historyPanel = null; await refresh(); status = 'Version restored'; }
    catch (e) { status = e.message; } finally { busy = false; }
  }

  async function loadTrash() {
    try { trash = await listTrash(); } catch (e) { status = e.message; }
  }
  async function restoreFromTrash(t) {
    busy = true;
    try { await restoreTrash(t.trash_path); await loadTrash(); status = 'Restored'; }
    catch (e) { status = e.message; } finally { busy = false; }
  }

  function togglePalette() { paletteOpen = !paletteOpen; paletteQuery = ''; }
  const paletteResults = $derived(
    paletteQuery
      ? allFiles.filter((f) => f.path.toLowerCase().includes(paletteQuery.toLowerCase())).slice(0, 8)
      : allFiles.slice(0, 8)
  );
  function paletteOpenFile(f) { paletteOpen = false; openPreview(f.path); }

  async function connectGitHub() {
    connecting = true;
    try {
      const d = await deviceStart();
      if (!d) { status = 'OAuth not configured.'; connecting = false; return; }
      device = d; pollLoop();
    } catch (e) { status = e.message; connecting = false; }
  }
  async function pollLoop() {
    if (!device) return;
    try {
      const res = await devicePoll(device.device_code);
      if (res.status === 'authorized') { device = null; connecting = false; status = 'GitHub connected ✓'; await refresh(); return; }
      if (res.status === 'denied' || res.status === 'error') { device = null; connecting = false; status = 'Auth failed.'; return; }
    } catch (e) { status = e.message; }
    setTimeout(pollLoop, (device?.interval || 5) * 1000);
  }
</script>

<div class="app">
  <aside class="sidebar" class:collapsed={!sidebarOpen}>
    <div class="brand">
      <span class="logo">🌥️</span>
      {#if sidebarOpen}<span class="brand-name">Nimbus</span>{/if}
    </div>
    <nav>
      <button class:active={view === 'drive'} onclick={() => setView('drive')}><span class="ic">📁</span>{#if sidebarOpen}Drive{/if}</button>
      <button class:active={view === 'chat'} onclick={() => setView('chat')}><span class="ic">💬</span>{#if sidebarOpen}Chat{/if}</button>
      <button class:active={view === 'trash'} onclick={() => setView('trash')}><span class="ic">🗑️</span>{#if sidebarOpen}Trash{/if}</button>
    </nav>
    {#if sidebarOpen && usage}
      <div class="usage">
        {#if usage.quota}
          <div class="usage-bar"><span style="width:{Math.min(100, (usage.used / usage.quota) * 100)}%"></span></div>
          <div class="usage-text">{formatSize(usage.used)} / {formatSize(usage.quota)}</div>
        {:else}
          <div class="usage-text">{formatSize(usage.used)} · {usage.count} files</div>
        {/if}
      </div>
    {/if}

    <div class="sidebar-foot">
      <button class="iconbtn" title="Command palette (Ctrl/⌘K)" onclick={togglePalette}>⌘K</button>
      <button class="iconbtn" title="Toggle theme" onclick={toggleTheme}>{theme === 'dark' ? '🌙' : '☀️'}</button>
      <button class="iconbtn" title="Collapse" onclick={() => (sidebarOpen = !sidebarOpen)}>{sidebarOpen ? '«' : '»'}</button>
    </div>
  </aside>

  <div class="main">
    <header class="topbar">
      {#if view === 'drive'}
        <div class="crumbs">
          <button class="crumb" onclick={goRoot}>Home</button>
          {#each breadcrumb as part, i}
            <span class="sep">/</span><button class="crumb" onclick={() => crumbTo(i)}>{part}</button>
          {/each}
        </div>
      {:else}
        <div class="crumbs"><strong style="text-transform:capitalize">{view}</strong></div>
      {/if}

      <div class="topbar-actions">
        <div class="searchbar">
          <input type="search" placeholder="Search semantically…" bind:value={query}
            onkeydown={(e) => e.key === 'Enter' && onSearch()} />
        </div>
        <div class="seg">
          <button class:on={viewMode === 'list'} onclick={() => setViewMode('list')} title="List">≣</button>
          <button class:on={viewMode === 'grid'} onclick={() => setViewMode('grid')} title="Grid">▦</button>
        </div>
        <button class="ghost" onclick={onSync} disabled={busy}>↻</button>
        <label class="primary">Upload<input type="file" multiple onchange={onInputChange} hidden /></label>
      </div>
    </header>

    {#if oauthAvailable}
      <div class="auth-banner">
        {#if device}
          Open <a href={device.verification_uri} target="_blank" rel="noreferrer">{device.verification_uri}</a>
          and enter <strong>{device.user_code}</strong> — waiting…
        {:else}
          <button class="ghost" onclick={connectGitHub} disabled={connecting}>🔗 Connect GitHub</button>
        {/if}
      </div>
    {/if}

    {#if status}<div class="status">{status}</div>{/if}

    <main class="content"
      role="region"
      class:dragging={dragging && view === 'drive'}
      ondragover={(e) => { if (view === 'drive') { e.preventDefault(); dragging = true; } }}
      ondragleave={() => (dragging = false)}
      ondrop={(e) => view === 'drive' && onDrop(e)}>

      {#if results !== null}
        <h3>Search results</h3>
        {#if results.length === 0}<p class="empty">No matches.</p>{/if}
        <ul class="rows">
          {#each results as hit}
            <li><button class="name" onclick={() => openPreview(hit.path)}>📄 {hit.path}</button>
              <span class="muted">{(hit.score * 100).toFixed(0)}%</span></li>
          {/each}
        </ul>
      {:else if view === 'drive'}
        {#if entries.length === 0}
          <div class="empty-state">
            <div class="big">📂</div>
            <p>This folder is empty. Drag files here or click <strong>Upload</strong>.</p>
          </div>
        {:else}
          <div class={viewMode === 'grid' ? 'grid' : 'rows-wrap'}>
            {#each entries as entry}
              {#if entry.kind === 'folder'}
                <div class="entry folder" class:card={viewMode === 'grid'}>
                  <button class="name" onclick={() => navigateInto(entry)}>📁 {basename(entry.path)}</button>
                </div>
              {:else}
                <div class="entry" class:card={viewMode === 'grid'}>
                  <button class="name" onclick={() => openPreview(entry.path)}>📄 {basename(entry.path)}</button>
                  <span class="meta">
                    <span class="muted">{formatSize(entry.size)}</span>
                    <span class="actions">
                      <a href={downloadUrl(entry.path)} download title="Download">↓</a>
                      <button title="Share" onclick={() => openShare(entry)}>🔗</button>
                      <button title="History" onclick={() => openHistory(entry)}>🕰️</button>
                      <button title="Rename" onclick={() => renameEntry(entry)}>✎</button>
                      <button title="Trash" onclick={() => deleteEntry(entry)}>🗑️</button>
                    </span>
                  </span>
                </div>
              {/if}
            {/each}
          </div>
        {/if}
      {:else if view === 'trash'}
        {#if trash.length === 0}<div class="empty-state"><div class="big">🗑️</div><p>Trash is empty.</p></div>{/if}
        <ul class="rows">
          {#each trash as t}
            <li><span class="name">📄 {t.original_path}</span>
              <button class="ghost sm" onclick={() => restoreFromTrash(t)}>Restore</button></li>
          {/each}
        </ul>
      {:else if view === 'chat'}
        <div class="chat-view">
          <div class="chat-input">
            <input type="text" placeholder="Ask a question about your files…" bind:value={chatQ}
              onkeydown={(e) => e.key === 'Enter' && onChat()} />
            <button class="primary" onclick={onChat} disabled={chatting}>{chatting ? 'Thinking…' : 'Ask AI'}</button>
          </div>
          {#if chatAnswer}
            <div class="answer">
              <p>{chatAnswer}</p>
              {#if chatSources.length}
                <p class="muted">Sources: {#each chatSources as s, i}<button class="link" onclick={() => openPreview(s)}>{basename(s)}</button>{i < chatSources.length - 1 ? ', ' : ''}{/each}</p>
              {/if}
            </div>
          {/if}
        </div>
      {/if}
    </main>
  </div>
</div>

<!-- Preview modal -->
{#if preview}
  <div class="backdrop" role="button" tabindex="0" onclick={() => (preview = null)} onkeydown={() => {}}>
    <div class="modal" role="dialog" tabindex="0" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
      <div class="modal-head"><span class="modal-title">{preview.path}</span>
        <a class="ghost-link" href={downloadUrl(preview.path)} download>Download</a>
        <button class="iconbtn" onclick={() => (preview = null)}>✕</button></div>
      <div class="modal-body">
        {#if preview.kind === 'image'}<img src={downloadUrl(preview.path)} alt={preview.path} />
        {:else if preview.kind === 'pdf'}<object data={downloadUrl(preview.path)} type="application/pdf" title={preview.path}><p>No inline PDF.</p></object>
        {:else if preview.kind === 'text'}<pre>{preview.text ?? 'Loading…'}</pre>
        {:else}<p class="empty">No preview. <a href={downloadUrl(preview.path)}>Download</a>.</p>{/if}
      </div>
    </div>
  </div>
{/if}

<!-- Share modal -->
{#if shareModal}
  <div class="backdrop" role="button" tabindex="0" onclick={() => (shareModal = null)} onkeydown={() => {}}>
    <div class="modal sm-modal" role="dialog" tabindex="0" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
      <div class="modal-head"><span class="modal-title">Share “{basename(shareModal.path)}”</span>
        <button class="iconbtn" onclick={() => (shareModal = null)}>✕</button></div>
      <div class="modal-body form">
        {#if !shareModal.result}
          <label>Password (optional)<input type="text" bind:value={shareModal.password} placeholder="none" /></label>
          <label>Expires in hours (optional)<input type="number" bind:value={shareModal.expires} placeholder="never" /></label>
          <button class="primary" onclick={doShare} disabled={busy}>Create link</button>
        {:else}
          <p class="muted">Anyone with this link {shareModal.password ? '(and the password) ' : ''}can download the file:</p>
          <input type="text" readonly value={shareModal.result} />
          <button class="primary" onclick={copyShare}>Copy link</button>
        {/if}
      </div>
    </div>
  </div>
{/if}

<!-- History panel -->
{#if historyPanel}
  <div class="backdrop" role="button" tabindex="0" onclick={() => (historyPanel = null)} onkeydown={() => {}}>
    <div class="modal" role="dialog" tabindex="0" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
      <div class="modal-head"><span class="modal-title">History — {basename(historyPanel.path)}</span>
        <button class="iconbtn" onclick={() => (historyPanel = null)}>✕</button></div>
      <div class="modal-body">
        {#if historyPanel.commits.length === 0}<p class="empty">No history.</p>{/if}
        <ul class="rows">
          {#each historyPanel.commits as c}
            <li><span class="name">{c.message}<br /><span class="muted">{c.date.slice(0, 10)} · {c.sha.slice(0, 7)}</span></span>
              <button class="ghost sm" onclick={() => restoreCommit(c.sha)}>Restore</button></li>
          {/each}
        </ul>
      </div>
    </div>
  </div>
{/if}

<!-- Command palette -->
{#if paletteOpen}
  <div class="backdrop palette-bd" role="button" tabindex="0" onclick={togglePalette} onkeydown={() => {}}>
    <div class="palette" role="dialog" tabindex="0" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
      <input class="palette-input" placeholder="Jump to a file…" bind:value={paletteQuery} autofocus
        onkeydown={(e) => e.key === 'Enter' && paletteResults[0] && paletteOpenFile(paletteResults[0])} />
      <ul class="palette-list">
        {#each paletteResults as f}
          <li><button onclick={() => paletteOpenFile(f)}>📄 {f.path}</button></li>
        {/each}
        {#if paletteResults.length === 0}<li class="muted pad">No files</li>{/if}
      </ul>
    </div>
  </div>
{/if}
