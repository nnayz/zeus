// Only immutable public application assets are cached. Never API/credential data.
const CACHE = 'zeus-companion-shell-v1';
const ASSETS = ['./', './index.html', './app.css', './app.js', './client.js', './model.js', './icon.svg', './manifest.webmanifest'];
const urls = new Set(ASSETS.map(path => new URL(path, self.registration.scope).href));
self.addEventListener('install', event => event.waitUntil(caches.open(CACHE).then(cache => cache.addAll(ASSETS))));
self.addEventListener('activate', event => event.waitUntil(caches.keys().then(keys => Promise.all(keys.filter(key => key.startsWith('zeus-companion-shell-') && key !== CACHE).map(key => caches.delete(key))))));
self.addEventListener('fetch', event => {
  const request = event.request;
  if (request.method !== 'GET' || request.headers.has('Authorization') || !urls.has(request.url)) return;
  event.respondWith(fetch(request).catch(() => caches.match(request)));
});
self.addEventListener('message', event => {
  if (event.data === 'delete-shell') event.waitUntil(caches.delete(CACHE));
});
