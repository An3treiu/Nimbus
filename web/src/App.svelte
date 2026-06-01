<script>
  import { onMount } from 'svelte';
  import { listFiles, uploadFile, downloadUrl, syncDrive, search, formatSize } from './api.js';

  let files = $state([]);
  let query = $state('');
  let results = $state(null); // null = not searching; [] = no hits
  let status = $state('');
  let busy = $state(false);
  let dragging = $state(false);

  async function refresh() {
    try {
      files = await listFiles();
    } catch (e) {
      status = e.message;
    }
  }

  onMount(refresh);

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
    } catch (e) {
      status = e.message;
    } finally {
      busy = false;
    }
  }

  function onInputChange(e) {
    handleFiles(e.target.files);
    e.target.value = '';
  }

  function onDrop(e) {
    e.preventDefault();
    dragging = false;
    handleFiles(e.dataTransfer.files);
  }

  async function onSync() {
    busy = true;
    status = 'Syncing from GitHub…';
    try {
      await syncDrive();
      await refresh();
      status = 'Synced';
    } catch (e) {
      status = e.message;
    } finally {
      busy = false;
    }
  }

  async function onSearch() {
    if (!query.trim()) {
      results = null;
      return;
    }
    busy = true;
    try {
      const hits = await search(query.trim());
      if (hits === null) {
        status = 'Semantic search is disabled (set NIMBUS_AI_PROVIDER).';
        results = null;
      } else {
        results = hits;
        status = `${hits.length} result(s)`;
      }
    } catch (e) {
      status = e.message;
    } finally {
      busy = false;
    }
  }

  function clearSearch() {
    query = '';
    results = null;
    status = '';
  }
</script>

<main>
  <header>
    <h1>🌥️ Nimbus</h1>
    <p class="tagline">Your files, in your own GitHub repo. Private by default.</p>
  </header>

  <section class="toolbar">
    <div class="searchbar">
      <input
        type="search"
        placeholder="Search your files semantically…"
        bind:value={query}
        onkeydown={(e) => e.key === 'Enter' && onSearch()}
      />
      <button onclick={onSearch} disabled={busy}>Search</button>
      {#if results !== null}
        <button class="ghost" onclick={clearSearch}>Clear</button>
      {/if}
    </div>
    <button class="ghost" onclick={onSync} disabled={busy}>↻ Sync</button>
  </section>

  <section
    class="dropzone"
    class:dragging
    role="button"
    tabindex="0"
    ondragover={(e) => { e.preventDefault(); dragging = true; }}
    ondragleave={() => (dragging = false)}
    ondrop={onDrop}
  >
    <p>Drag files here, or</p>
    <label class="upload-btn">
      Choose files
      <input type="file" multiple onchange={onInputChange} hidden />
    </label>
  </section>

  {#if status}
    <p class="status">{status}</p>
  {/if}

  {#if results !== null}
    <h2>Search results</h2>
    {#if results.length === 0}
      <p class="empty">No matches.</p>
    {:else}
      <ul class="files">
        {#each results as hit}
          <li>
            <a href={downloadUrl(hit.path)} class="name">{hit.path}</a>
            <span class="score">{(hit.score * 100).toFixed(0)}% match</span>
          </li>
        {/each}
      </ul>
    {/if}
  {:else}
    <h2>Files</h2>
    {#if files.length === 0}
      <p class="empty">No files yet. Upload something above.</p>
    {:else}
      <ul class="files">
        {#each files as file}
          <li>
            <a href={downloadUrl(file.path)} class="name">{file.path}</a>
            <span class="size">{formatSize(file.size)}</span>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}

  <footer>
    <span>Nimbus · open-source, self-hosted, AI-native cloud drive</span>
  </footer>
</main>
