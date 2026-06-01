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

export function formatSize(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`;
}
