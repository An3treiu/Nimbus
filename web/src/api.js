// Thin client for the Nimbus HTTP API (same-origin).

// Encode each path segment but keep "/" so it matches the `*path` route.
function encodePath(path) {
  return path.split('/').map(encodeURIComponent).join('/');
}

export async function listFiles() {
  const r = await fetch('/api/files');
  if (!r.ok) throw new Error(`list failed (${r.status})`);
  return r.json();
}

export async function uploadFile(path, blob) {
  const r = await fetch(`/api/files/${encodePath(path)}`, { method: 'POST', body: blob });
  if (!r.ok) throw new Error(`upload failed (${r.status})`);
  return r.json();
}

export function downloadUrl(path) {
  return `/api/files/${encodePath(path)}`;
}

export async function syncDrive() {
  const r = await fetch('/api/sync', { method: 'POST' });
  if (!r.ok) throw new Error(`sync failed (${r.status})`);
}

// Returns an array of hits, or null if semantic search is disabled (501).
export async function search(query, k = 10) {
  const r = await fetch(`/api/search?q=${encodeURIComponent(query)}&k=${k}`);
  if (r.status === 501) return null;
  if (!r.ok) throw new Error(`search failed (${r.status})`);
  return r.json();
}

// Chat with your files. Returns { answer, sources } or null if chat is disabled.
export async function chatWithFiles(question) {
  const r = await fetch('/api/chat', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ question }),
  });
  if (r.status === 501) return null;
  if (!r.ok) throw new Error(`chat failed (${r.status})`);
  return r.json();
}

export async function authStatus() {
  const r = await fetch('/api/auth/status');
  if (!r.ok) return { oauth_available: false };
  return r.json();
}

export async function deviceStart() {
  const r = await fetch('/api/auth/device/start', { method: 'POST' });
  if (r.status === 501) return null;
  if (!r.ok) throw new Error(`device start failed (${r.status})`);
  return r.json();
}

export async function devicePoll(deviceCode) {
  const r = await fetch('/api/auth/device/poll', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ device_code: deviceCode }),
  });
  if (!r.ok) throw new Error(`poll failed (${r.status})`);
  return r.json();
}

export async function fetchText(path) {
  const r = await fetch(downloadUrl(path));
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
