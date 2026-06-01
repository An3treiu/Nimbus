<script>
  import { onMount } from 'svelte';
  import {
    listFiles, uploadFile, downloadUrl, syncDrive, search,
    chatWithFiles, fetchText, previewKind, formatSize,
    authStatus, deviceStart, devicePoll,
  } from './api.js';

  let files = $state([]);
  let query = $state('');
  let results = $state(null);
  let status = $state('');
  let busy = $state(false);
  let dragging = $state(false);

  // Preview modal state
  let preview = $state(null); // { path, kind, text? }

  // Chat state
  let chatQ = $state('');
  let chatAnswer = $state(null);
  let chatSources = $state([]);
  let chatting = $state(false);

  // GitHub OAuth (device flow)
  let oauthAvailable = $state(false);
  let device = $state(null); // { user_code, verification_uri, device_code, interval }
  let connecting = $state(false);

  async function refresh() {
    try { files = await listFiles(); } catch (e) { status = e.message; }
  }

  onMount(async () => {
    await refresh();
    try { oauthAvailable = (await authStatus()).oauth_available; } catch {}
  });

  async function connectGitHub() {
    connecting = true; status = '';
    try {
      const d = await deviceStart();
      if (!d) { status = 'OAuth not configured on the server.'; connecting = false; return; }
      device = d;
      pollLoop();
    } catch (e) { status = e.message; connecting = false; }
  }

  async function pollLoop() {
    if (!device) return;
    try {
      const res = await devicePoll(device.device_code);
      if (res.status === 'authorized') {
        device = null; connecting = false; status = 'GitHub connected ✓';
        await refresh();
        return;
      }
      if (res.status === 'denied' || res.status === 'error') {
        device = null; connecting = false; status = 'GitHub authorization failed.';
        return;
      }
    } catch (e) { status = e.message; }
    setTimeout(pollLoop, (device?.interval || 5) * 1000);
  }

  async function handleFiles(fileList) {
    if (!fileList || fileList.length === 0) return;
    busy = true;
    try {
      for (const f of fileList) {
        status = `Uploading ${f.name}…`;
        await uploadFile(f.name, f);
      }
      status = `Uploaded ${fileList.length} file(s)`;
      await refresh();
    } catch (e) { status = e.message; } finally { busy = false; }
  }

  function onInputChange(e) { handleFiles(e.target.files); e.target.value = ''; }
  function onDrop(e) { e.preventDefault(); dragging = false; handleFiles(e.dataTransfer.files); }

  async function onSync() {
    busy = true; status = 'Syncing from GitHub…';
    try { await syncDrive(); await refresh(); status = 'Synced'; }
    catch (e) { status = e.message; } finally { busy = false; }
  }

  async function onSearch() {
    if (!query.trim()) { results = null; return; }
    busy = true;
    try {
      const hits = await search(query.trim());
      if (hits === null) { status = 'Search disabled (set NIMBUS_AI_PROVIDER).'; results = null; }
      else { results = hits; status = `${hits.length} result(s)`; }
    } catch (e) { status = e.message; } finally { busy = false; }
  }

  function clearSearch() { query = ''; results = null; status = ''; }

  async function openPreview(path) {
    const kind = previewKind(path);
    preview = { path, kind, text: null };
    if (kind === 'text') {
      try { preview = { path, kind, text: await fetchText(path) }; }
      catch (e) { preview = { path, kind: 'none', text: null }; status = e.message; }
    }
  }
  function closePreview() { preview = null; }

  async function onChat() {
    if (!chatQ.trim()) return;
    chatting = true; chatAnswer = null; chatSources = [];
    try {
      const res = await chatWithFiles(chatQ.trim());
      if (res === null) { status = 'Chat disabled (set NIMBUS_AI_PROVIDER).'; }
      else { chatAnswer = res.answer; chatSources = res.sources || []; }
    } catch (e) { status = e.message; } finally { chatting = false; }
  }
</script>

<main>
  <header>
    <h1>🌥️ Nimbus</h1>
    <p class="tagline">Your files, in your own GitHub repo. Private by default.</p>
  </header>

  {#if oauthAvailable}
    <section class="auth">
      {#if device}
        <p>Open <a href={device.verification_uri} target="_blank" rel="noreferrer">{device.verification_uri}</a>
          and enter code <strong>{device.user_code}</strong> — waiting for authorization…</p>
      {:else}
        <button class="ghost" onclick={connectGitHub} disabled={connecting}>🔗 Connect GitHub</button>
      {/if}
    </section>
  {/if}

  <section class="toolbar">
    <div class="searchbar">
      <input type="search" placeholder="Search your files semantically…"
        bind:value={query} onkeydown={(e) => e.key === 'Enter' && onSearch()} />
      <button onclick={onSearch} disabled={busy}>Search</button>
      {#if results !== null}<button class="ghost" onclick={clearSearch}>Clear</button>{/if}
    </div>
    <button class="ghost" onclick={onSync} disabled={busy}>↻ Sync</button>
  </section>

  <section class="dropzone" class:dragging role="button" tabindex="0"
    ondragover={(e) => { e.preventDefault(); dragging = true; }}
    ondragleave={() => (dragging = false)} ondrop={onDrop}>
    <p>Drag files here, or</p>
    <label class="upload-btn">Choose files<input type="file" multiple onchange={onInputChange} hidden /></label>
  </section>

  <section class="chat">
    <div class="chat-input">
      <input type="text" placeholder="Ask a question about your files…"
        bind:value={chatQ} onkeydown={(e) => e.key === 'Enter' && onChat()} />
      <button onclick={onChat} disabled={chatting}>{chatting ? 'Thinking…' : 'Ask AI'}</button>
    </div>
    {#if chatAnswer}
      <div class="answer">
        <p>{chatAnswer}</p>
        {#if chatSources.length}
          <p class="sources">Sources: {#each chatSources as s, i}<a href={downloadUrl(s)}>{s}</a>{i < chatSources.length - 1 ? ', ' : ''}{/each}</p>
        {/if}
      </div>
    {/if}
  </section>

  {#if status}<p class="status">{status}</p>{/if}

  {#if results !== null}
    <h2>Search results</h2>
    {#if results.length === 0}<p class="empty">No matches.</p>{:else}
      <ul class="files">
        {#each results as hit}
          <li>
            <button class="name link" onclick={() => openPreview(hit.path)}>{hit.path}</button>
            <span class="score">{(hit.score * 100).toFixed(0)}% match</span>
          </li>
        {/each}
      </ul>
    {/if}
  {:else}
    <h2>Files</h2>
    {#if files.length === 0}<p class="empty">No files yet. Upload something above.</p>{:else}
      <ul class="files">
        {#each files as file}
          <li>
            <button class="name link" onclick={() => openPreview(file.path)}>{file.path}</button>
            <span class="actions">
              <span class="size">{formatSize(file.size)}</span>
              <a href={downloadUrl(file.path)} download class="dl">↓</a>
            </span>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}

  <footer><span>Nimbus · open-source, self-hosted, AI-native cloud drive</span></footer>
</main>

{#if preview}
  <div class="modal-backdrop" role="button" tabindex="0"
    onclick={closePreview} onkeydown={(e) => e.key === 'Escape' && closePreview()}>
    <div class="modal" role="dialog" tabindex="0" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
      <div class="modal-head">
        <span class="modal-title">{preview.path}</span>
        <a href={downloadUrl(preview.path)} download class="ghost-link">Download</a>
        <button class="ghost" onclick={closePreview}>✕</button>
      </div>
      <div class="modal-body">
        {#if preview.kind === 'image'}
          <img src={downloadUrl(preview.path)} alt={preview.path} />
        {:else if preview.kind === 'pdf'}
          <object data={downloadUrl(preview.path)} type="application/pdf" title={preview.path}>
            <p>PDF preview unavailable. <a href={downloadUrl(preview.path)}>Download</a>.</p>
          </object>
        {:else if preview.kind === 'text'}
          <pre>{preview.text ?? 'Loading…'}</pre>
        {:else}
          <p class="empty">No preview for this file type. <a href={downloadUrl(preview.path)}>Download it</a>.</p>
        {/if}
      </div>
    </div>
  </div>
{/if}
