// Thin client for the Nimbus HTTP API (same-origin).

// --- Auth token (only needed when the server sets NIMBUS_ADMIN_TOKEN) ---
// Stored in localStorage for fetch() Authorization headers, and mirrored to a
// cookie so plain <img>/<a download> requests (which can't set headers) pass auth.
export function setToken(t) {
  localStorage.setItem('nimbus_token', t);
  document.cookie = `nimbus_token=${encodeURIComponent(t)}; path=/; SameSite=Strict`;
}
function token() {
  return localStorage.getItem('nimbus_token') || '';
}
// Re-apply the cookie on load if we already have a token.
if (typeof document !== 'undefined' && token()) setToken(token());

async function apiFetch(url, opts = {}) {
  const headers = new Headers(opts.headers || {});
  const t = token();
  if (t) headers.set('Authorization', `Bearer ${t}`);
  const r = await fetch(url, { ...opts, headers });
  if (r.status === 401) {
    const entered = prompt('This Nimbus instance requires an access token:');
    if (entered) {
      setToken(entered.trim());
      return apiFetch(url, opts);
    }
  }
  return r;
}

// Encode each path segment but keep "/" so it matches the `*path` route.
function encodePath(path) {
  return path.split('/').map(encodeURIComponent).join('/');
}

export async function listFiles(prefix) {
  const q = prefix !== undefined ? `?prefix=${encodeURIComponent(prefix)}` : '';
  const r = await apiFetch(`/api/files${q}`);
  if (!r.ok) throw new Error(`list failed (${r.status})`);
  return r.json();
}

export async function uploadFile(path, blob) {
  const r = await apiFetch(`/api/files/${encodePath(path)}`, { method: 'POST', body: blob });
  if (!r.ok) throw new Error(`upload failed (${r.status})`);
  return r.json();
}

export async function deleteFile(path) {
  const r = await apiFetch(`/api/files/${encodePath(path)}`, { method: 'DELETE' });
  if (!r.ok) throw new Error(`delete failed (${r.status})`);
}

export async function moveFile(from, to) {
  const r = await apiFetch('/api/move', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ from, to }),
  });
  if (!r.ok) throw new Error(`move failed (${r.status})`);
}

export function downloadUrl(path) {
  return `/api/files/${encodePath(path)}`;
}

export async function syncDrive() {
  const r = await apiFetch('/api/sync', { method: 'POST' });
  if (!r.ok) throw new Error(`sync failed (${r.status})`);
}

// Returns an array of hits, or null if semantic search is disabled (501).
export async function search(query, k = 10) {
  const r = await apiFetch(`/api/search?q=${encodeURIComponent(query)}&k=${k}`);
  if (r.status === 501) return null;
  if (!r.ok) throw new Error(`search failed (${r.status})`);
  return r.json();
}

// Chat with your files. Returns { answer, sources } or null if chat is disabled.
export async function chatWithFiles(question) {
  const r = await apiFetch('/api/chat', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ question }),
  });
  if (r.status === 501) return null;
  if (!r.ok) throw new Error(`chat failed (${r.status})`);
  return r.json();
}

export async function authStatus() {
  const r = await apiFetch('/api/auth/status');
  if (!r.ok) return { oauth_available: false };
  return r.json();
}

export async function deviceStart() {
  const r = await apiFetch('/api/auth/device/start', { method: 'POST' });
  if (r.status === 501) return null;
  if (!r.ok) throw new Error(`device start failed (${r.status})`);
  return r.json();
}

export async function devicePoll(deviceCode) {
  const r = await apiFetch('/api/auth/device/poll', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ device_code: deviceCode }),
  });
  if (!r.ok) throw new Error(`poll failed (${r.status})`);
  return r.json();
}

export async function listTrash() {
  const r = await apiFetch('/api/trash');
  if (!r.ok) throw new Error(`trash list failed (${r.status})`);
  return r.json();
}

export async function restoreTrash(trashPath) {
  const r = await apiFetch('/api/trash/restore', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ trash_path: trashPath }),
  });
  if (!r.ok) throw new Error(`restore failed (${r.status})`);
}

export async function fileHistory(path) {
  const r = await apiFetch(`/api/history/${encodePath(path)}`);
  if (!r.ok) throw new Error(`history failed (${r.status})`);
  return r.json();
}

export async function restoreVersion(path, commit) {
  const r = await apiFetch('/api/restore', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ path, commit }),
  });
  if (!r.ok) throw new Error(`restore version failed (${r.status})`);
}

export async function createShare(path, password, expiresInSecs) {
  const body = { path };
  if (password) body.password = password;
  if (expiresInSecs) body.expires_in_secs = expiresInSecs;
  const r = await apiFetch('/api/share', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!r.ok) throw new Error(`share failed (${r.status})`);
  return r.json();
}

export async function fetchText(path) {
  const r = await apiFetch(downloadUrl(path));
  if (!r.ok) throw new Error(`load failed (${r.status})`);
  return r.text();
}

const IMAGE_EXT = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'avif', 'bmp'];
const TEXT_EXT = ['txt', 'md', 'markdown', 'json', 'js', 'ts', 'rs', 'py', 'go',
  'html', 'css', 'toml', 'yaml', 'yml', 'csv', 'log', 'sh', 'c', 'cpp', 'java'];

export function previewKind(path) {
  const ext = path.split('.').pop().toLowerCase();
  if (IMAGE_EXT.includes(ext)) return 'image';
  if (ext === 'pdf') return 'pdf';
  if (TEXT_EXT.includes(ext)) return 'text';
  return 'none';
}

export function formatSize(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`;
}
