// Privacy-first page analytics for the Pointr website.
//
// Sends a handful of events straight to PostHog's capture endpoint, the same
// way the desktop app's telemetry does: no third-party script, no cookies, no
// persistent visitor ID, no IP or location, no session recording.
//
//   $pageview       every page load (with referrer + utm_* so social posts show up)
//   download_click  the Windows installer links (which button, which version)
//   outbound_click  links leaving the site (host only, never the full URL)
//   scroll_depth    50% and 90% of the page, once each
//
// The visitor ID is random, lives in sessionStorage (gone when the tab closes)
// and is not linked to anything. So counts are visits, not unique people.
//
// Setup: paste your PostHog *project* key (the public phc_ one, write-only by
// design) into KEY. While it's empty this file does nothing at all.
//
// Skipping your own visits: open the site once with ?notrack=1 in each browser
// you use. It's remembered in that browser; ?notrack=0 turns tracking back on.
//
// Testing: add ?analytics_debug=1 to any URL (works on localhost too). Events
// are sent for real and each one is also printed in the browser console, and if
// nothing is sent the console says why. Open DevTools > Network and filter on
// "capture" to see the requests themselves.
(function () {
  'use strict';

  var KEY = 'phc_t6kLQCRmm2HKnjaQmnJ4uFNyCsyGFwrB9gUbxj2rgNmK';
  var HOST = 'https://us.i.posthog.com';

  var me = document.currentScript;
  // Test hooks only: point at a local capture server without editing this file.
  if (me && me.getAttribute('data-key')) KEY = me.getAttribute('data-key');
  if (me && me.getAttribute('data-host')) HOST = me.getAttribute('data-host');
  var testing = !!(me && me.getAttribute('data-host'));

  var debug = false;
  try { debug = new URLSearchParams(location.search).get('analytics_debug') === '1'; } catch (e) {}
  if (debug) testing = true; // allow localhost / file:// while debugging
  function skip(why) {
    if (debug) console.info('[pointr-analytics] not sending: ' + why);
  }

  if (!KEY) { skip('no project key set in analytics.js'); return; }

  function store(kind) {
    try { return window[kind]; } catch (e) { return null; }
  }
  var local = store('localStorage');
  var session = store('sessionStorage');

  // ?notrack=1 excludes this browser from now on; ?notrack=0 undoes it.
  try {
    var flag = new URLSearchParams(location.search).get('notrack');
    if (local && flag === '1') local.setItem('pointr_notrack', '1');
    if (local && flag === '0') local.removeItem('pointr_notrack');
    if (local && local.getItem('pointr_notrack') === '1') { skip('this browser is excluded (?notrack=1); open any page with ?notrack=0 to undo'); return; }
  } catch (e) {}

  // Respect Do Not Track and Global Privacy Control.
  if (navigator.doNotTrack === '1' || window.doNotTrack === '1' || navigator.globalPrivacyControl) { skip('Do Not Track / Global Privacy Control is on in this browser'); return; }

  var host = location.hostname;
  if (!testing && (location.protocol === 'file:' || host === 'localhost' || host === '127.0.0.1')) { skip('local page (add ?analytics_debug=1 to test from here)'); return; }

  var ua = navigator.userAgent || '';
  if (/bot|crawl|spider|slurp|preview|lighthouse|facebookexternalhit|linkedinbot|twitterbot/i.test(ua)) { skip('user agent looks like a bot'); return; }

  // UUIDv7 (time-ordered), which is what PostHog expects for session ids.
  function uuidv7() {
    var b = new Uint8Array(16);
    crypto.getRandomValues(b);
    var t = Date.now();
    b[0] = Math.floor(t / 1099511627776) & 255;
    b[1] = Math.floor(t / 4294967296) & 255;
    b[2] = (t >>> 24) & 255;
    b[3] = (t >>> 16) & 255;
    b[4] = (t >>> 8) & 255;
    b[5] = t & 255;
    b[6] = (b[6] & 15) | 112;
    b[8] = (b[8] & 63) | 128;
    var h = Array.prototype.map.call(b, function (x) { return ('0' + x.toString(16)).slice(-2); }).join('');
    return h.slice(0, 8) + '-' + h.slice(8, 12) + '-' + h.slice(12, 16) + '-' + h.slice(16, 20) + '-' + h.slice(20);
  }

  // One visit = one tab session, ended by closing the tab or 30 idle minutes.
  var IDLE_MS = 30 * 60 * 1000;
  function visit() {
    var now = Date.now();
    var id = null;
    var last = 0;
    if (session) {
      try {
        id = session.getItem('pointr_vid');
        last = Number(session.getItem('pointr_vt')) || 0;
      } catch (e) {}
    }
    if (!id || now - last > IDLE_MS) {
      id = uuidv7();
      if (session) { try { session.removeItem('pointr_utm'); } catch (e) {} }
    }
    if (session) {
      try {
        session.setItem('pointr_vid', id);
        session.setItem('pointr_vt', String(now));
      } catch (e) {}
    }
    return id;
  }

  // utm_* from the landing URL, remembered for the rest of the visit so a
  // click through to the setup guide is still credited to the same post.
  var UTM = ['utm_source', 'utm_medium', 'utm_campaign', 'utm_content', 'utm_term'];
  function utm() {
    var out = {};
    var q = new URLSearchParams(location.search);
    var any = false;
    UTM.forEach(function (k) {
      if (q.get(k)) { out[k] = q.get(k); any = true; }
    });
    if (any) {
      if (session) { try { session.setItem('pointr_utm', JSON.stringify(out)); } catch (e) {} }
      return out;
    }
    if (session) {
      try { return JSON.parse(session.getItem('pointr_utm') || '{}'); } catch (e) {}
    }
    return {};
  }

  function refHost() {
    try { return document.referrer ? new URL(document.referrer).hostname : ''; } catch (e) { return ''; }
  }

  function send(event, extra) {
    var id = visit();
    var w = window.innerWidth || 0;
    var props = {
      $lib: 'pointr-web',
      $lib_version: '1',
      $process_person_profile: false, // anonymous events, no person records
      $geoip_disable: true,
      $ip: '0.0.0.0',
      $session_id: id,
      $current_url: location.href.split('#')[0],
      $host: host,
      $pathname: location.pathname,
      $referrer: document.referrer || '$direct',
      $referring_domain: refHost() || '$direct',
      $raw_user_agent: ua,
      $device_type: /Mobi|Android|iPhone|iPad/i.test(ua) ? 'Mobile' : 'Desktop',
      $viewport_width: w,
      $viewport_height: window.innerHeight || 0,
      $screen_width: screen.width,
      $screen_height: screen.height,
      title: document.title
    };
    var u = utm();
    Object.keys(u).forEach(function (k) { props[k] = u[k]; });
    if (extra) Object.keys(extra).forEach(function (k) { props[k] = extra[k]; });

    if (debug) console.log('[pointr-analytics] ' + event, props);
    var body = JSON.stringify({
      api_key: KEY,
      event: event,
      distinct_id: id,
      properties: props,
      timestamp: new Date().toISOString()
    });
    var url = HOST + '/capture/';
    try {
      // text/plain keeps it a "simple" request, so no CORS preflight.
      if (navigator.sendBeacon && navigator.sendBeacon(url, new Blob([body], { type: 'text/plain' }))) return;
    } catch (e) {}
    try { fetch(url, { method: 'POST', body: body, headers: { 'Content-Type': 'text/plain' }, keepalive: true }); } catch (e) {}
  }

  send('$pageview');

  function where(a) {
    if (a.closest('.mobile-drawer-bottom, .mobile-drawer')) return 'drawer';
    if (a.closest('.site-header, header')) return 'nav';
    if (a.closest('.hero-section')) return 'hero';
    if (a.closest('.site-footer, footer')) return 'footer';
    return 'page';
  }

  document.addEventListener('click', function (e) {
    var a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
    if (!a) return;
    var href = a.getAttribute('href') || '';
    var m = /releases\/download\/(v[\d.]+)\//.exec(href);
    if (m) {
      send('download_click', { version: m[1], location: where(a) });
      return;
    }
    try {
      var to = new URL(a.href, location.href);
      if (to.hostname && to.hostname !== host) send('outbound_click', { destination_host: to.hostname, location: where(a) });
    } catch (err) {}
  }, true);

  var seen = {};
  function onScroll() {
    var doc = document.documentElement;
    var max = doc.scrollHeight - window.innerHeight;
    if (max <= 0) return;
    var pct = ((window.scrollY || doc.scrollTop) / max) * 100;
    [50, 90].forEach(function (d) {
      if (pct >= d && !seen[d]) { seen[d] = true; send('scroll_depth', { depth: d }); }
    });
  }
  var ticking = false;
  window.addEventListener('scroll', function () {
    if (ticking) return;
    ticking = true;
    setTimeout(function () { ticking = false; onScroll(); }, 200);
  }, { passive: true });
})();
