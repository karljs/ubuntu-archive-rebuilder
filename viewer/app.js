// Rebuild Experiments Report Viewer

const DATA_BASE_URL = './data';
const SQL_JS_CDN = 'https://cdn.jsdelivr.net/npm/sql.js@1.12.0/dist/';

let sqlDb = null;
let batches = [];        // all batches from DB
let visibleBatches = []; // batches after header filters applied
let currentBatch = null;
let currentBatchData = null;
let sortColumn = 'package';
let sortDirection = 'asc';
let successRateChart = null;
let outcomeAreaChart = null;
let failureCatsChart = null;
let catSelectedBatches = []; // ordered list of batch ids for Categories tab

function el(id) { return document.getElementById(id); }

// ── SQL helper ──

function dbQuery(sql, params) {
    var stmt = sqlDb.prepare(sql);
    if (params) stmt.bind(params);
    var rows = [];
    while (stmt.step()) rows.push(stmt.getAsObject());
    stmt.free();
    return rows;
}

// ── Custom dropdown helpers ──

function initDropdown(containerId, onChange) {
    var dd = el(containerId);
    if (!dd) return;
    var toggle = dd.querySelector('.dropdown-toggle');
    var menu = dd.querySelector('.dropdown-menu');

    toggle.addEventListener('click', function(e) {
        e.stopPropagation();
        document.querySelectorAll('.dropdown.open').forEach(function(d) {
            if (d !== dd) d.classList.remove('open');
        });
        dd.classList.toggle('open');
    });

    menu.addEventListener('click', function(e) {
        var li = e.target.closest('li');
        if (!li) return;
        e.stopPropagation();
        var val = li.getAttribute('data-value');
        toggle.textContent = li.textContent;
        dd.dataset.value = val;
        menu.querySelectorAll('li').forEach(function(item) {
            item.classList.toggle('selected', item === li);
        });
        dd.classList.remove('open');
        if (onChange) onChange(val);
    });
}

function setDropdownOptions(containerId, options) {
    var dd = el(containerId);
    if (!dd) return;
    var menu = dd.querySelector('.dropdown-menu');
    var toggle = dd.querySelector('.dropdown-toggle');
    menu.innerHTML = options.map(function(o, i) {
        return '<li data-value="' + escapeAttr(o.value) + '"' + (i === 0 ? ' class="selected"' : '') + '>' + escapeHtml(o.label) + '</li>';
    }).join('');
    if (options.length > 0) {
        toggle.textContent = options[0].label;
        dd.dataset.value = options[0].value;
    }
}

function getDropdownValue(containerId) {
    var dd = el(containerId);
    return dd ? (dd.dataset.value || '') : '';
}

document.addEventListener('click', function() {
    document.querySelectorAll('.dropdown.open').forEach(function(d) {
        d.classList.remove('open');
    });
});

// ── Init ──

if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
} else {
    init();
}

async function init() {
    try {
        var SQL = await initSqlJs({ locateFile: function(f) { return SQL_JS_CDN + f; } });
        var buf = await fetch(DATA_BASE_URL + '/rebuild.db').then(function(r) {
            if (!r.ok) throw new Error('rebuild.db not found \u2014 run: rebuild-pipeline export');
            return r.arrayBuffer();
        });
        sqlDb = new SQL.Database(new Uint8Array(buf));

        var overlay = el('loading-overlay');
        if (overlay) overlay.classList.add('hidden');

        loadBatches();
        setupEventListeners();
    } catch (err) {
        console.error('Init failed:', err);
        var overlay = el('loading-overlay');
        if (overlay) overlay.innerHTML = '<p class="load-error">Failed to load database: ' + escapeHtml(String(err.message || err)) + '</p>';
    }
}

// ════════════════════════════════════════════════
// Tab navigation
// ════════════════════════════════════════════════

function switchTab(tabName, pushHistory) {
    document.querySelectorAll('.tab-btn').forEach(function(btn) {
        btn.classList.toggle('active', btn.dataset.tab === tabName);
    });
    document.querySelectorAll('.tab-panel').forEach(function(p) {
        p.classList.toggle('active', p.id === 'tab-' + tabName);
    });

    // Lazy-render tab content.
    if (tabName === 'trends') renderTrends();
    if (tabName === 'categories') renderCategoriesTab();
    if (tabName === 'compare') populateCompareDropdowns();

    if (pushHistory !== false) pushView({ tab: tabName });
}

function getActiveTab() {
    var btn = document.querySelector('.tab-btn.active');
    return btn ? btn.dataset.tab : 'builds';
}

// ── Data loading ──

function loadBatches() {
    var batchRows = dbQuery(
        "SELECT id, name, compiler_type, compiler_version, series, profile_name, started_at, finished_at " +
        "FROM batches ORDER BY started_at DESC"
    );

    var statRows = dbQuery(
        "SELECT batch_id, status, COUNT(*) AS count FROM builds GROUP BY batch_id, status"
    );
    var statsMap = {};
    for (var i = 0; i < statRows.length; i++) {
        var r = statRows[i];
        if (!statsMap[r.batch_id]) {
            statsMap[r.batch_id] = { total: 0, succeeded: 0, failed: 0, dep_wait: 0, timeout: 0 };
        }
        var s = statsMap[r.batch_id];
        var count = Number(r.count);
        s.total += count;
        if (r.status === 'succeeded') s.succeeded = count;
        else if (r.status === 'failed') s.failed = count;
        else if (r.status === 'dep_wait') s.dep_wait = count;
        else if (r.status === 'timeout') s.timeout = count;
    }

    batches = batchRows.map(function(row) {
        return {
            id: row.id,
            name: row.name,
            compiler_type: row.compiler_type,
            compiler_version: row.compiler_version,
            series: row.series,
            profile_name: row.profile_name,
            started_at: row.started_at,
            finished_at: row.finished_at,
            stats: statsMap[row.id] || { total: 0, succeeded: 0, failed: 0, dep_wait: 0, timeout: 0 }
        };
    });

    // Populate header filter dropdowns with distinct values.
    var compilers = unique(batches.map(function(b) { return b.compiler_type; })).sort();
    var versions  = unique(batches.map(function(b) { return b.compiler_version; })).sort();
    var series    = unique(batches.map(function(b) { return b.series; })).sort();

    setDropdownOptions('filter-compiler-dd', [{value: '', label: 'All'}].concat(compilers.map(function(v) { return {value: v, label: v}; })));
    setDropdownOptions('filter-version-dd',  [{value: '', label: 'All'}].concat(versions.map(function(v)  { return {value: v, label: v}; })));
    setDropdownOptions('filter-series-dd',   [{value: '', label: 'All'}].concat(series.map(function(v)    { return {value: v, label: v}; })));

    applyBatchFilters();
}

function unique(arr) {
    return arr.filter(function(v, i, a) { return a.indexOf(v) === i; });
}

function applyBatchFilters() {
    var compiler = getDropdownValue('filter-compiler-dd');
    var version  = getDropdownValue('filter-version-dd');
    var series   = getDropdownValue('filter-series-dd');

    visibleBatches = batches.filter(function(b) {
        if (compiler && b.compiler_type    !== compiler) return false;
        if (version  && b.compiler_version !== version)  return false;
        if (series   && b.series           !== series)   return false;
        return true;
    });

    setDropdownOptions('batch-select-dd', visibleBatches.map(function(b) {
        return { value: b.id, label: b.name + ' (' + b.stats.succeeded + '/' + b.stats.total + ')' };
    }));

    // Select first visible batch if current is no longer visible.
    if (visibleBatches.length > 0) {
        var stillVisible = currentBatch && visibleBatches.some(function(b) { return b.id === currentBatch.id; });
        if (!stillVisible) selectBatch(visibleBatches[0].id);
    }
}

function loadBatchData(batchId) {
    var buildRows = dbQuery(
        "SELECT id, source_package AS package, version, status, " +
        "build_duration_seconds AS duration_seconds, peak_memory_mb " +
        "FROM builds WHERE batch_id = ? ORDER BY source_package",
        [batchId]
    );

    var countRows = dbQuery(
        "SELECT build_id, COUNT(*) AS count FROM build_findings " +
        "WHERE build_id IN (SELECT id FROM builds WHERE batch_id = ?) " +
        "GROUP BY build_id",
        [batchId]
    );
    var countMap = {};
    for (var i = 0; i < countRows.length; i++) {
        countMap[countRows[i].build_id] = Number(countRows[i].count);
    }

    var summaryRows = dbQuery(
        "SELECT bf.category, COUNT(*) AS count " +
        "FROM build_findings bf JOIN builds b ON bf.build_id = b.id " +
        "WHERE b.batch_id = ? GROUP BY bf.category ORDER BY count DESC",
        [batchId]
    );

    return {
        builds: buildRows.map(function(row) {
            return {
                id: row.id,
                package: row.package,
                version: row.version,
                status: row.status,
                duration_seconds: row.duration_seconds,
                peak_memory_mb: row.peak_memory_mb,
                finding_count: countMap[row.id] || 0
            };
        }),
        finding_summary: summaryRows.map(function(row) {
            return { category: row.category, count: Number(row.count) };
        })
    };
}

function selectBatch(batchId, pushHistory) {
    currentBatch = visibleBatches.find(function(b) { return b.id === batchId; });
    if (!currentBatch) currentBatch = batches.find(function(b) { return b.id === batchId; });
    if (!currentBatch) return;
    currentBatchData = loadBatchData(batchId);
    renderStatusBar();
    renderFindings();
    renderBuildsTable();
    if (pushHistory !== false) pushView({ tab: 'builds', batchId: batchId });
}

// ── Event listeners ──

function setupEventListeners() {
    // Tab buttons.
    document.querySelectorAll('.tab-btn').forEach(function(btn) {
        btn.addEventListener('click', function() {
            switchTab(this.dataset.tab);
        });
    });

    initDropdown('batch-select-dd', function(val) { selectBatch(val); });
    initDropdown('status-filter-dd', function() { renderBuildsTable(); });
    initDropdown('compare-batch-a-dd');
    initDropdown('compare-batch-b-dd');
    initDropdown('cat-add-batch-dd');

    // Builds tab batch filters.
    initDropdown('filter-compiler-dd', function() { applyBatchFilters(); });
    initDropdown('filter-version-dd',  function() { applyBatchFilters(); });
    initDropdown('filter-series-dd',   function() { applyBatchFilters(); });

    var rc = el('run-compare');
    if (rc) rc.addEventListener('click', runComparison);

    var fi = el('filter-input');
    if (fi) fi.addEventListener('input', renderBuildsTable);

    var mc = el('modal-close');
    if (mc) mc.addEventListener('click', closeModal);
    var lmc = el('log-modal-close');
    if (lmc) lmc.addEventListener('click', closeLogModal);

    var m = el('modal');
    if (m) m.addEventListener('click', function(e) { if (e.target === this) closeModal(); });
    var lm = el('log-modal');
    if (lm) lm.addEventListener('click', function(e) { if (e.target === this) closeLogModal(); });

    document.querySelectorAll('th[data-sort]').forEach(function(th) {
        th.addEventListener('click', function() { handleSort(this.dataset.sort); });
    });

    var ls = el('log-search');
    if (ls) ls.addEventListener('input', handleLogSearch);

    // Trend mode radio buttons.
    document.querySelectorAll('input[name="trend-mode"]').forEach(function(r) {
        r.addEventListener('change', function() {
            if (getActiveTab() === 'trends') renderTrends();
        });
    });

    // Categories add button.
    var cab = el('cat-add-btn');
    if (cab) cab.addEventListener('click', catAddBatch);

    document.addEventListener('keydown', function(e) {
        if (e.key === 'Escape') {
            var logModal = el('log-modal');
            var modal = el('modal');
            if (logModal && !logModal.classList.contains('hidden')) { closeLogModal(); return; }
            if (modal && !modal.classList.contains('hidden')) { closeModal(); return; }
        }
    });
}

function closeModal() {
    var m = el('modal');
    if (m) m.classList.add('hidden');
}

function closeLogModal() {
    var lm = el('log-modal');
    if (lm) lm.classList.add('hidden');
}

// ════════════════════════════════════════════════
// Builds tab rendering
// ════════════════════════════════════════════════

function renderStatusBar() {
    var b = currentBatch;
    var s = b.stats;
    var rate = s.total > 0 ? ((s.succeeded / s.total) * 100).toFixed(0) : 0;
    var started = new Date(b.started_at).toLocaleString();

    var totalSecs = 0;
    if (currentBatchData && currentBatchData.builds) {
        for (var i = 0; i < currentBatchData.builds.length; i++) {
            totalSecs += currentBatchData.builds[i].duration_seconds || 0;
        }
    }

    var sb = el('status-bar');
    if (sb) sb.innerHTML =
        '<span class="s-pass">' + s.succeeded + ' passed</span>' +
        '<span class="s-fail">' + s.failed + ' failed</span>' +
        (s.timeout > 0 ? '<span class="s-timeout">' + s.timeout + ' timeout</span>' : '') +
        (s.dep_wait > 0 ? '<span class="s-depwait">' + s.dep_wait + ' dep-wait</span>' : '') +
        '<span>' + s.total + ' total</span>' +
        '<span><span class="rate-bar"><span class="rate-fill" style="width:' + rate + '%"></span></span> ' + rate + '%</span>' +
        '<span>' + fmtDuration(totalSecs) + ' total build time</span>' +
        '<span class="batch-meta">' + escapeHtml(b.compiler_type + ' ' + b.compiler_version + ' \u00b7 ' + b.series + ' \u00b7 ' + started) + '</span>';
}

function renderFindings() {
    var fc = el('findings-content');
    if (!fc) return;

    var findings = (currentBatchData && currentBatchData.finding_summary) || [];

    var unanalyzed = 0;
    if (currentBatchData && currentBatchData.builds) {
        for (var i = 0; i < currentBatchData.builds.length; i++) {
            var build = currentBatchData.builds[i];
            if (build.status !== 'succeeded' && !build.finding_count) unanalyzed++;
        }
    }

    if (findings.length === 0 && unanalyzed === 0) {
        fc.innerHTML = '<p class="muted">No issues in this batch.</p>';
        return;
    }

    var total = 0;
    for (var i = 0; i < findings.length; i++) total += findings[i].count;

    var html = '';
    for (var i = 0; i < findings.length; i++) {
        var f = findings[i];
        html += '<div class="findings-bar-item">' +
            '<span class="findings-bar-count">' + f.count + '</span>' +
            '<span class="findings-bar-label" title="' + escapeHtml(f.category) + '">' + escapeHtml(f.category) + '</span>' +
            '</div>';
    }
    if (unanalyzed > 0) {
        html += '<div class="findings-bar-item findings-bar-unanalyzed">' +
            '<span class="findings-bar-count">' + unanalyzed + '</span>' +
            '<span class="findings-bar-label">Unanalyzed (build failed before analysis)</span>' +
            '</div>';
        total += unanalyzed;
    }
    html = '<p class="muted" style="margin-bottom:6px">' + total + ' issues across ' + (findings.length + (unanalyzed > 0 ? 1 : 0)) + ' categories</p>' + html;
    fc.innerHTML = html;
}

function diffsCell(b) {
    if (b.finding_count > 0) return String(b.finding_count);
    if (b.status === 'succeeded') return '<span class="cell-hint" data-hint="Build succeeded with no issues detected">0</span>';
    if (b.status === 'failed' || b.status === 'timeout' || b.status === 'dep_wait')
        return '<span class="cell-hint" data-hint="Build did not complete; no analysis was performed">n/a</span>';
    return '-';
}

function renderBuildsTable() {
    if (!currentBatchData) return;
    var tbody = el('builds-tbody');
    if (!tbody) return;

    var builds = currentBatchData.builds.slice();
    var fi = el('filter-input');
    var filt = fi ? fi.value.toLowerCase() : '';
    var statFilt = getDropdownValue('status-filter-dd');

    builds = builds.filter(function(b) {
        if (filt && b.package.toLowerCase().indexOf(filt) === -1) return false;
        if (statFilt && b.status !== statFilt) return false;
        return true;
    });

    builds.sort(function(a, b) {
        var av, bv;
        switch (sortColumn) {
            case 'package':  av = a.package;           bv = b.package;           break;
            case 'status':   av = a.status;            bv = b.status;            break;
            case 'duration': av = a.duration_seconds || 0; bv = b.duration_seconds || 0; break;
            case 'memory':   av = a.peak_memory_mb || 0;   bv = b.peak_memory_mb || 0;   break;
            case 'findings': av = a.finding_count || 0;    bv = b.finding_count || 0;    break;
            default:         av = a.package;           bv = b.package;
        }
        if (typeof av === 'string') {
            return sortDirection === 'asc' ? av.localeCompare(bv) : bv.localeCompare(av);
        }
        return sortDirection === 'asc' ? av - bv : bv - av;
    });

    document.querySelectorAll('th[data-sort]').forEach(function(th) {
        th.classList.remove('sort-asc', 'sort-desc');
        if (th.dataset.sort === sortColumn) {
            th.classList.add(sortDirection === 'asc' ? 'sort-asc' : 'sort-desc');
        }
    });

    var html = '';
    for (var i = 0; i < builds.length; i++) {
        var b = builds[i];
        html += '<tr>' +
            '<td><span class="pkg-name">' + escapeHtml(b.package) + '</span></td>' +
            '<td><span class="st st-' + b.status + '">' + b.status + '</span></td>' +
            '<td class="num mono">' + (b.duration_seconds ? fmtDuration(b.duration_seconds) : '-') + '</td>' +
            '<td class="num mono">' + (b.peak_memory_mb ? b.peak_memory_mb + ' MB' : '-') + '</td>' +
            '<td class="num">' + diffsCell(b) + '</td>' +
            '<td>' +
                (b.finding_count > 0 ? '<button class="btn-link" data-action="details" data-id="' + b.id + '">details</button> ' : '') +
                '<button class="btn-link" data-action="log" data-id="' + b.id + '" data-pkg="' + escapeAttr(b.package) + '">log</button>' +
            '</td>' +
            '</tr>';
    }
    tbody.innerHTML = html;
}

document.addEventListener('click', function(e) {
    if (e.target.closest('.dropdown')) return;
    var btn = e.target.closest('[data-action]');
    if (!btn) return;
    var action = btn.getAttribute('data-action');
    var id = btn.getAttribute('data-id');
    if (action === 'details') showBuildDetails(id);
    if (action === 'log') showBuildLog(id, btn.getAttribute('data-pkg'));
});

function handleSort(col) {
    if (sortColumn === col) {
        sortDirection = sortDirection === 'asc' ? 'desc' : 'asc';
    } else {
        sortColumn = col;
        sortDirection = 'asc';
    }
    renderBuildsTable();
}

// ════════════════════════════════════════════════
// Utilities
// ════════════════════════════════════════════════

function fmtDuration(s) {
    if (s < 60) return Math.round(s) + 's';
    var m = Math.floor(s / 60);
    var sec = Math.round(s % 60);
    if (m < 60) return m + 'm' + (sec > 0 ? sec + 's' : '');
    var h = Math.floor(m / 60);
    var rm = m % 60;
    return h + 'h' + (rm > 0 ? rm + 'm' : '');
}

function escapeHtml(text) {
    var d = document.createElement('div');
    d.textContent = String(text);
    return d.innerHTML;
}

function escapeAttr(text) {
    return String(text).replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/'/g, '&#39;').replace(/</g, '&lt;');
}

function escapeRegex(s) {
    return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function fmtDelta(delta, fmt, threshold) {
    if (delta == null) return '<span class="delta-same">-</span>';
    var abs = Math.abs(delta);
    if (abs < threshold) return '<span class="delta-same">\u00b1' + fmt(abs) + '</span>';
    if (delta > 0) return '<span class="delta-worse">+' + fmt(abs) + '</span>';
    return '<span class="delta-better">\u2212' + fmt(abs) + '</span>';
}

/** Short batch label: use profile_name or truncate full name. */
function shortBatchLabel(b) {
    return b.profile_name || b.name;
}

// ── Build details modal ──

function showBuildDetails(buildId) {
    var findings = dbQuery(
        "SELECT category, description, excerpt, line_number " +
        "FROM build_findings WHERE build_id = ? ORDER BY line_number",
        [buildId]
    );

    var packageName = '';
    if (currentBatchData) {
        var build = currentBatchData.builds.find(function(b) { return b.id === buildId; });
        if (build) packageName = build.package;
    }

    var mt = el('modal-title');
    var mb = el('modal-body');
    if (mt) mt.textContent = packageName + ' \u2014 Findings';

    if (findings.length === 0) {
        if (mb) mb.innerHTML = '<p class="muted">No findings.</p>';
    } else {
        var html = '';
        for (var i = 0; i < findings.length; i++) {
            var f = findings[i];
            html += '<div class="finding-detail">' +
                '<h4>' + escapeHtml(f.category) + '</h4>' +
                '<p>' + escapeHtml(f.description) + '</p>' +
                (f.line_number ? '<p class="muted">Line ' + f.line_number + '</p>' : '') +
                '<pre>' + escapeHtml(f.excerpt) + '</pre>' +
                '</div>';
        }
        if (mb) mb.innerHTML = html;
    }
    var m = el('modal');
    if (m) m.classList.remove('hidden');
}

// ── Log viewer ──

var currentLogText = '';

async function showBuildLog(buildId, packageName) {
    try {
        var r = await fetch(DATA_BASE_URL + '/logs/' + buildId + '.log');
        if (!r.ok) throw new Error('Log not found');
        currentLogText = await r.text();

        var lt = el('log-modal-title');
        if (lt) lt.textContent = packageName + ' \u2014 Build Log';
        var ls = el('log-search');
        if (ls) ls.value = '';
        var lsc = el('log-search-count');
        if (lsc) lsc.textContent = '';
        renderLog(currentLogText);
        var lm = el('log-modal');
        if (lm) lm.classList.remove('hidden');
        setTimeout(function() { if (ls) ls.focus(); }, 100);
    } catch (err) {
        console.error('Log load failed:', err);
    }
}

function renderLog(text, searchTerm) {
    var lc = el('log-content');
    if (!lc) return;

    var lines = text.split('\n');
    var numWidth = String(lines.length).length;
    var hitCount = 0;

    var html = '';
    for (var i = 0; i < lines.length; i++) {
        var num = String(i + 1);
        while (num.length < numWidth) num = ' ' + num;
        var content = escapeHtml(lines[i]);

        if (searchTerm) {
            var escaped = escapeHtml(searchTerm);
            var re = new RegExp(escapeRegex(escaped), 'gi');
            content = content.replace(re, function(m) {
                hitCount++;
                return '<span class="search-hit">' + m + '</span>';
            });
        }

        html += '<div class="log-line"><span class="line-num">' + num + '</span><span class="line-text">' + content + '</span></div>';
    }

    lc.innerHTML = html;

    var lsc = el('log-search-count');
    if (searchTerm) {
        if (lsc) lsc.textContent = hitCount + ' match' + (hitCount !== 1 ? 'es' : '');
        var first = lc.querySelector('.search-hit');
        if (first) first.scrollIntoView({ block: 'center' });
    } else {
        if (lsc) lsc.textContent = '';
    }
}

function handleLogSearch() {
    var ls = el('log-search');
    var term = ls ? ls.value.trim() : '';
    renderLog(currentLogText, term || null);
}

// ════════════════════════════════════════════════
// Browser history
// ════════════════════════════════════════════════

var _historyInitialised = false;

function pushView(state) {
    if (!_historyInitialised) {
        _historyInitialised = true;
        history.replaceState(state, '');
    } else {
        history.pushState(state, '');
    }
}

function restoreView(state) {
    if (!state) return;
    var tab = state.tab || 'builds';
    switchTab(tab, false);
    if (tab === 'builds' && state.batchId) selectBatch(state.batchId, false);
}

window.addEventListener('popstate', function(e) {
    restoreView(e.state);
});

// ════════════════════════════════════════════════
// Trends tab
// ════════════════════════════════════════════════

// Palette for chart lines/areas.
var CHART_COLORS = [
    '#0969da', '#cf222e', '#1a7f37', '#8250df', '#9a6700',
    '#bc4c00', '#0055cc', '#116329', '#953800', '#3d8bcd',
    '#e05d44', '#2ea44f', '#6f42c1', '#d4a017', '#1b7c83'
];

// Known Ubuntu series in release order, oldest first.
var SERIES_ORDER = ['focal', 'groovy', 'hirsute', 'impish', 'jammy', 'kinetic',
                    'lunar', 'mantic', 'noble', 'oracular', 'plucky', 'questing',
                    'resolute', 'stonking'];

function seriesRank(s) {
    var i = SERIES_ORDER.indexOf(s);
    return i === -1 ? 999 : i;
}

function getTrendMode() {
    var r = document.querySelector('input[name="trend-mode"]:checked');
    return r ? r.value : 'version';
}

/** Derive the list of available profile checkboxes for the Trends config panel. */
function getTrendProfiles() {
    // Collect distinct profile_name values from batches.
    var profileNames = unique(batches.map(function(b) { return b.profile_name; })).sort();
    return profileNames;
}

/** Return which profile_names are currently checked. */
function getCheckedProfiles() {
    var checks = document.querySelectorAll('#trends-profile-checks input[type="checkbox"]');
    var checked = [];
    checks.forEach(function(cb) {
        if (cb.checked) checked.push(cb.value);
    });
    return checked;
}

/** Populate the profile checkboxes in the trends sidebar. */
function renderTrendProfileChecks() {
    var container = el('trends-profile-checks');
    if (!container) return;

    var profiles = getTrendProfiles();
    // Preserve current checked state if already rendered.
    var prevChecked = getCheckedProfiles();
    var isFirstRender = container.children.length === 0;

    var html = '';
    for (var i = 0; i < profiles.length; i++) {
        var pname = profiles[i];
        var checked = isFirstRender ? true : prevChecked.indexOf(pname) !== -1;
        var color = CHART_COLORS[i % CHART_COLORS.length];
        html += '<label>' +
            '<input type="checkbox" value="' + escapeAttr(pname) + '"' + (checked ? ' checked' : '') + '>' +
            '<span class="check-swatch" style="background:' + color + '"></span>' +
            escapeHtml(pname) +
            '</label>';
    }
    container.innerHTML = html;

    // Attach change listeners.
    container.querySelectorAll('input[type="checkbox"]').forEach(function(cb) {
        cb.addEventListener('change', function() {
            renderTrendCharts();
        });
    });
}

function renderTrends() {
    renderTrendProfileChecks();
    renderTrendCharts();
}

function renderTrendCharts() {
    var mode = getTrendMode();
    var checkedProfiles = getCheckedProfiles();

    // Update description.
    var desc = el('trends-rate-desc');
    if (desc) {
        var descText = {
            version: 'Success rate across compiler versions. One line per selected profile. Gaps indicate missing data.',
            series: 'Success rate across Ubuntu series. One line per selected profile.'
        };
        desc.textContent = descText[mode] || '';
    }

    renderSuccessRateChart(mode, checkedProfiles);
    renderOutcomeAreaChart(mode, checkedProfiles);
}

/**
 * Build the integer version range for the X axis (with gaps).
 * E.g. versions [14, 18] -> labels [14, 15, 16, 17, 18].
 */
function buildVersionRange(versions) {
    if (versions.length === 0) return [];
    var nums = versions.map(function(v) { return parseInt(v, 10); }).filter(function(n) { return !isNaN(n); });
    if (nums.length === 0) return versions; // fallback if non-numeric
    var min = Math.min.apply(null, nums);
    var max = Math.max.apply(null, nums);
    var range = [];
    for (var i = min; i <= max; i++) range.push(String(i));
    return range;
}

/** Filter batches by checked profile names. */
function filterByProfiles(batchList, checkedProfiles) {
    if (!checkedProfiles.length) return [];
    return batchList.filter(function(b) {
        return checkedProfiles.indexOf(b.profile_name) !== -1;
    });
}

/** Get the color for a profile_name (consistent index). */
function profileColor(profileName) {
    var all = getTrendProfiles();
    var idx = all.indexOf(profileName);
    return CHART_COLORS[(idx >= 0 ? idx : 0) % CHART_COLORS.length];
}

function renderSuccessRateChart(mode, checkedProfiles) {
    var ctx = el('chart-success-rate');
    if (!ctx) return;
    if (successRateChart) successRateChart.destroy();

    var filtered = filterByProfiles(batches, checkedProfiles);
    if (!filtered.length) {
        successRateChart = null;
        return;
    }

    var td = buildSuccessRateDatasets(mode, filtered);

    successRateChart = new Chart(ctx, {
        type: 'line',
        data: { labels: td.xLabels, datasets: td.datasets },
        options: {
            responsive: true,
            maintainAspectRatio: false,
            scales: {
                y: { min: 0, max: 100, ticks: { callback: function(v) { return v + '%'; } } },
                x: { title: { display: true, text: td.xTitle }, ticks: { maxRotation: 40 } }
            },
            plugins: {
                legend: { display: td.datasets.length > 1, position: 'bottom',
                          labels: { boxWidth: 12, font: { size: 11 } } },
                tooltip: { callbacks: { label: function(ctx) {
                    return (td.datasets.length > 1 ? ctx.dataset.label + ': ' : '') + ctx.parsed.y + '%';
                }}}
            }
        }
    });
}

function buildSuccessRateDatasets(mode, filtered) {
    if (mode === 'version') {
        var versions = unique(filtered.map(function(b) { return b.compiler_version; }))
            .sort(function(a, b) { return parseFloat(a) - parseFloat(b); });
        var xLabels = buildVersionRange(versions);

        // Group by profile_name.
        var profileNames = unique(filtered.map(function(b) { return b.profile_name; })).sort();
        var datasets = profileNames.map(function(pname) {
            var color = profileColor(pname);
            var data = xLabels.map(function(v) {
                var matching = filtered.filter(function(b) {
                    return b.profile_name === pname && b.compiler_version === v;
                }).sort(function(a, b) { return b.started_at < a.started_at ? -1 : 1; });
                if (!matching.length) return null;
                var stats = matching[0].stats;
                return stats.total > 0 ? parseFloat(((stats.succeeded / stats.total) * 100).toFixed(1)) : 0;
            });
            return {
                label: pname, data: data,
                borderColor: color, backgroundColor: color + '22',
                pointRadius: 4, tension: 0.2, spanGaps: false
            };
        });
        return { datasets: datasets, xLabels: xLabels, xTitle: 'Compiler version' };

    } else {
        // Series mode.
        var seriesList = unique(filtered.map(function(b) { return b.series; }))
            .sort(function(a, b) { return seriesRank(a) - seriesRank(b); });
        var profileNames2 = unique(filtered.map(function(b) { return b.profile_name; })).sort();

        var datasets2 = profileNames2.map(function(pname) {
            var color = profileColor(pname);
            var data = seriesList.map(function(s) {
                var matching = filtered.filter(function(b) {
                    return b.series === s && b.profile_name === pname;
                }).sort(function(a, b) { return b.started_at < a.started_at ? -1 : 1; });
                if (!matching.length) return null;
                var stats = matching[0].stats;
                return stats.total > 0 ? parseFloat(((stats.succeeded / stats.total) * 100).toFixed(1)) : 0;
            });
            return {
                label: pname, data: data,
                borderColor: color, backgroundColor: color + '22',
                pointRadius: 4, tension: 0.2, spanGaps: false
            };
        });
        return { datasets: datasets2, xLabels: seriesList, xTitle: 'Ubuntu series' };
    }
}

/**
 * Stacked area chart showing outcome breakdown (succeeded, each failure category, timeout, dep_wait).
 */
function renderOutcomeAreaChart(mode, checkedProfiles) {
    var ctx = el('chart-outcome-area');
    if (!ctx) return;
    if (outcomeAreaChart) outcomeAreaChart.destroy();

    var filtered = filterByProfiles(batches, checkedProfiles);
    if (!filtered.length) {
        outcomeAreaChart = null;
        return;
    }

    var td = buildOutcomeDatasets(mode, filtered);

    outcomeAreaChart = new Chart(ctx, {
        type: 'line',
        data: { labels: td.xLabels, datasets: td.datasets },
        options: {
            responsive: true,
            maintainAspectRatio: false,
            scales: {
                y: { stacked: true, min: 0, ticks: { callback: function(v) { return v + '%'; } },
                     title: { display: true, text: '% of packages' } },
                x: { title: { display: true, text: td.xTitle }, ticks: { maxRotation: 40 } }
            },
            plugins: {
                legend: { position: 'bottom', labels: { boxWidth: 12, font: { size: 11 } } },
                tooltip: {
                    mode: 'index',
                    callbacks: {
                        label: function(ctx) {
                            return ctx.dataset.label + ': ' + (ctx.parsed.y != null ? ctx.parsed.y.toFixed(1) + '%' : 'n/a');
                        }
                    }
                }
            },
            interaction: { mode: 'index', intersect: false }
        }
    });
}

var OUTCOME_COLORS = {
    succeeded: '#1a7f37',
    dep_wait:  '#0969da',
    timeout:   '#9a6700',
    failed:    '#cf222e'
};

function buildOutcomeDatasets(mode, filtered) {
    // For each profile, build a "virtual" batch per x-axis tick.
    // If multiple profiles are checked, aggregate.
    // We show percentages of total across all selected profiles at each x-tick.

    var xLabels, getBatchesForTick;

    if (mode === 'version') {
        var versions = unique(filtered.map(function(b) { return b.compiler_version; }))
            .sort(function(a, b) { return parseFloat(a) - parseFloat(b); });
        xLabels = buildVersionRange(versions);
        getBatchesForTick = function(tickVal) {
            return filtered.filter(function(b) { return b.compiler_version === tickVal; });
        };
    } else {
        xLabels = unique(filtered.map(function(b) { return b.series; }))
            .sort(function(a, b) { return seriesRank(a) - seriesRank(b); });
        getBatchesForTick = function(tickVal) {
            return filtered.filter(function(b) { return b.series === tickVal; });
        };
    }

    // For each tick, pick the most recent batch per profile, then aggregate stats.
    var aggregated = xLabels.map(function(tick) {
        var tickBatches = getBatchesForTick(tick);
        // Deduplicate: keep most recent per profile_name.
        var byProfile = {};
        tickBatches.forEach(function(b) {
            if (!byProfile[b.profile_name] || b.started_at > byProfile[b.profile_name].started_at) {
                byProfile[b.profile_name] = b;
            }
        });
        var selected = Object.values(byProfile);
        var agg = { total: 0, succeeded: 0, failed: 0, timeout: 0, dep_wait: 0 };
        selected.forEach(function(b) {
            agg.total += b.stats.total;
            agg.succeeded += b.stats.succeeded;
            agg.failed += b.stats.failed;
            agg.timeout += b.stats.timeout;
            agg.dep_wait += b.stats.dep_wait;
        });
        return agg;
    });

    // Build stacked datasets (order: succeeded at bottom, then dep_wait, timeout, failed on top).
    var statuses = ['succeeded', 'dep_wait', 'timeout', 'failed'];
    var statusLabels = { succeeded: 'Succeeded', dep_wait: 'Dep-wait', timeout: 'Timeout', failed: 'Failed' };

    var datasets = statuses.map(function(st) {
        var data = aggregated.map(function(agg) {
            if (agg.total === 0) return null;
            return parseFloat(((agg[st] / agg.total) * 100).toFixed(1));
        });
        return {
            label: statusLabels[st],
            data: data,
            fill: true,
            borderColor: OUTCOME_COLORS[st],
            backgroundColor: OUTCOME_COLORS[st] + '66',
            pointRadius: 3,
            tension: 0.2,
            spanGaps: false
        };
    });

    return {
        datasets: datasets,
        xLabels: xLabels,
        xTitle: mode === 'version' ? 'Compiler version' : 'Ubuntu series'
    };
}


// ════════════════════════════════════════════════
// Categories tab
// ════════════════════════════════════════════════

function renderCategoriesTab() {
    populateCatAddDropdown();
    renderCatBatchList();
    renderCatChart();
}

function populateCatAddDropdown() {
    var opts = batches.map(function(b) {
        return { value: b.id, label: shortBatchLabel(b) };
    });
    setDropdownOptions('cat-add-batch-dd', opts);
}

function catAddBatch() {
    var id = getDropdownValue('cat-add-batch-dd');
    if (!id) return;
    if (catSelectedBatches.indexOf(id) !== -1) return; // already present
    catSelectedBatches.push(id);
    renderCatBatchList();
    renderCatChart();
}

function catRemoveBatch(id) {
    catSelectedBatches = catSelectedBatches.filter(function(x) { return x !== id; });
    renderCatBatchList();
    renderCatChart();
}

function renderCatBatchList() {
    var list = el('cat-batch-list');
    var emptyMsg = el('cat-empty-msg');
    if (!list) return;

    if (catSelectedBatches.length === 0) {
        list.innerHTML = '';
        if (emptyMsg) emptyMsg.style.display = '';
        return;
    }
    if (emptyMsg) emptyMsg.style.display = 'none';

    var html = '';
    for (var i = 0; i < catSelectedBatches.length; i++) {
        var bid = catSelectedBatches[i];
        var b = batches.find(function(x) { return x.id === bid; });
        var label = b ? shortBatchLabel(b) : bid;
        html += '<li draggable="true" data-batch-id="' + escapeAttr(bid) + '">' +
            '<span class="drag-handle">\u2261</span>' +
            '<span class="cat-batch-name" title="' + escapeAttr(b ? b.name : bid) + '">' + escapeHtml(label) + '</span>' +
            '<button class="btn-icon cat-batch-remove" data-remove-id="' + escapeAttr(bid) + '" title="Remove">&times;</button>' +
            '</li>';
    }
    list.innerHTML = html;

    // Attach remove handlers.
    list.querySelectorAll('.cat-batch-remove').forEach(function(btn) {
        btn.addEventListener('click', function() {
            catRemoveBatch(this.getAttribute('data-remove-id'));
        });
    });

    // Drag-and-drop reordering.
    setupCatDragDrop(list);
}

function setupCatDragDrop(list) {
    var dragItem = null;

    list.querySelectorAll('li').forEach(function(li) {
        li.addEventListener('dragstart', function(e) {
            dragItem = this;
            this.classList.add('dragging');
            e.dataTransfer.effectAllowed = 'move';
            e.dataTransfer.setData('text/plain', ''); // required for Firefox
        });

        li.addEventListener('dragend', function() {
            this.classList.remove('dragging');
            dragItem = null;
        });

        li.addEventListener('dragover', function(e) {
            e.preventDefault();
            e.dataTransfer.dropEffect = 'move';
        });

        li.addEventListener('drop', function(e) {
            e.preventDefault();
            if (!dragItem || dragItem === this) return;

            // Reorder catSelectedBatches.
            var fromId = dragItem.getAttribute('data-batch-id');
            var toId = this.getAttribute('data-batch-id');
            var fromIdx = catSelectedBatches.indexOf(fromId);
            var toIdx = catSelectedBatches.indexOf(toId);
            if (fromIdx === -1 || toIdx === -1) return;

            catSelectedBatches.splice(fromIdx, 1);
            catSelectedBatches.splice(toIdx, 0, fromId);

            renderCatBatchList();
            renderCatChart();
        });
    });
}

function renderCatChart() {
    var ctx = el('chart-failure-cats');
    if (!ctx) return;
    if (failureCatsChart) { failureCatsChart.destroy(); failureCatsChart = null; }

    if (catSelectedBatches.length === 0) return;

    // Fetch finding data for selected batches.
    var batchIds = catSelectedBatches.map(function(id) { return "'" + id + "'"; }).join(',');
    var rows = dbQuery(
        "SELECT b.batch_id, bf.category, COUNT(*) AS count " +
        "FROM build_findings bf JOIN builds b ON bf.build_id = b.id " +
        "WHERE b.batch_id IN (" + batchIds + ") " +
        "GROUP BY b.batch_id, bf.category"
    );

    // Determine top categories across all selected batches.
    var catTotals = {};
    for (var i = 0; i < rows.length; i++) {
        var cat = rows[i].category;
        catTotals[cat] = (catTotals[cat] || 0) + Number(rows[i].count);
    }
    var topCats = Object.keys(catTotals).sort(function(a, b) {
        return catTotals[b] - catTotals[a];
    }).slice(0, 12);

    if (!topCats.length) return;

    // Build labels from selected batches in order.
    var labels = catSelectedBatches.map(function(bid) {
        var b = batches.find(function(x) { return x.id === bid; });
        return b ? shortBatchLabel(b) : bid;
    });

    var datasets = topCats.map(function(cat, ci) {
        var countByBatch = {};
        for (var i = 0; i < rows.length; i++) {
            if (rows[i].category === cat) countByBatch[rows[i].batch_id] = Number(rows[i].count);
        }
        return {
            label: cat,
            data: catSelectedBatches.map(function(bid) { return countByBatch[bid] || 0; }),
            backgroundColor: CHART_COLORS[ci % CHART_COLORS.length]
        };
    });

    failureCatsChart = new Chart(ctx, {
        type: 'bar',
        data: { labels: labels, datasets: datasets },
        options: {
            responsive: true, maintainAspectRatio: false,
            scales: {
                x: { stacked: true, ticks: { maxRotation: 40 } },
                y: { stacked: true, beginAtZero: true, ticks: { precision: 0 } }
            },
            plugins: { legend: { position: 'bottom', labels: { boxWidth: 12, font: { size: 11 } } } }
        }
    });
}


// ════════════════════════════════════════════════
// Compare tab
// ════════════════════════════════════════════════

function populateCompareDropdowns() {
    var opts = batches.map(function(b) { return { value: b.id, label: shortBatchLabel(b) }; });
    setDropdownOptions('compare-batch-a-dd', opts);
    setDropdownOptions('compare-batch-b-dd', opts);
}

function runComparison() {
    var aId = getDropdownValue('compare-batch-a-dd');
    var bId = getDropdownValue('compare-batch-b-dd');
    if (aId === bId) { alert('Select two different batches.'); return; }

    var batchA = batches.find(function(b) { return b.id === aId; });
    var batchB = batches.find(function(b) { return b.id === bId; });
    if (!batchA || !batchB) return;

    var dataA = loadBatchData(aId);
    var dataB = loadBatchData(bId);

    renderComparison(
        Object.assign({}, batchA, dataA),
        Object.assign({}, batchB, dataB)
    );
}

function renderComparison(a, b) {
    var content = el('compare-content');
    if (!content) return;

    var labelA = shortBatchLabel(a);
    var labelB = shortBatchLabel(b);

    var mA = {};
    for (var i = 0; i < a.builds.length; i++) mA[a.builds[i].package] = a.builds[i];
    var mB = {};
    for (var i = 0; i < b.builds.length; i++) mB[b.builds[i].package] = b.builds[i];

    var allPkgs = {};
    for (var k in mA) allPkgs[k] = true;
    for (var k in mB) allPkgs[k] = true;

    var changed = [], added = [], removed = [], same = [];
    for (var pkg in allPkgs) {
        var ba = mA[pkg], bb = mB[pkg];
        if (!ba) added.push({ package: pkg, b: bb });
        else if (!bb) removed.push({ package: pkg, a: ba });
        else if (ba.status !== bb.status) changed.push({ package: pkg, a: ba, b: bb });
        else same.push({ package: pkg, a: ba, b: bb });
    }

    var sortPkg = function(x, y) { return x.package.localeCompare(y.package); };
    changed.sort(sortPkg); added.sort(sortPkg); removed.sort(sortPkg); same.sort(sortPkg);

    // Summary section.
    var html = '<div class="compare-section">' +
        '<h3>' + escapeHtml(labelA) + ' vs ' + escapeHtml(labelB) + '</h3>' +
        '<div class="compare-summary">' +
            '<div class="compare-stat"><div class="compare-stat-value">' + changed.length + '</div><div class="compare-stat-label">Changed</div></div>' +
            '<div class="compare-stat"><div class="compare-stat-value">' + added.length + '</div><div class="compare-stat-label">New in B</div></div>' +
            '<div class="compare-stat"><div class="compare-stat-value">' + removed.length + '</div><div class="compare-stat-label">Removed</div></div>' +
            '<div class="compare-stat"><div class="compare-stat-value">' + same.length + '</div><div class="compare-stat-label">Same</div></div>' +
        '</div>' +
        '</div>';

    // Status changes section.
    if (changed.length > 0) {
        html += '<div class="compare-section">' +
            '<h3>Status Changes</h3>' +
            '<table><thead><tr>' +
            '<th>Package</th>' +
            '<th class="col-label-a" title="' + escapeAttr(a.name) + '">' + escapeHtml(labelA) + '</th>' +
            '<th class="col-label-b" title="' + escapeAttr(b.name) + '">' + escapeHtml(labelB) + '</th>' +
            '</tr></thead><tbody>';
        for (var i = 0; i < changed.length; i++) {
            var c = changed[i];
            html += '<tr class="compare-changed">' +
                '<td><span class="pkg-name">' + escapeHtml(c.package) + '</span></td>' +
                '<td><span class="st st-' + c.a.status + '">' + c.a.status + '</span></td>' +
                '<td><span class="st st-' + c.b.status + '">' + c.b.status + '</span></td>' +
                '</tr>';
        }
        html += '</tbody></table></div>';
    }

    // Resource usage: split into two separate tables.
    var resources = [];
    for (var pkg in mA) {
        var ra = mA[pkg], rb = mB[pkg];
        if (!rb) continue;
        var hasDur = ra.duration_seconds != null && rb.duration_seconds != null;
        var hasMem = ra.peak_memory_mb  != null && rb.peak_memory_mb  != null;
        if (!hasDur && !hasMem) continue;
        resources.push({
            package: pkg,
            durA: ra.duration_seconds, durB: rb.duration_seconds,
            memA: ra.peak_memory_mb,  memB: rb.peak_memory_mb,
            deltaDur: hasDur ? rb.duration_seconds - ra.duration_seconds : null,
            deltaMem: hasMem ? rb.peak_memory_mb  - ra.peak_memory_mb  : null
        });
    }

    if (resources.length > 0) {
        // Build time table.
        var durResources = resources.filter(function(r) { return r.deltaDur != null; })
            .sort(function(x, y) { return Math.abs(y.deltaDur) - Math.abs(x.deltaDur); });

        if (durResources.length > 0) {
            html += '<div class="compare-section">' +
                '<h3>Build Time</h3>' +
                '<table><thead><tr>' +
                '<th>Package</th>' +
                '<th class="num col-label-a" title="' + escapeAttr(a.name) + '">' + escapeHtml(labelA) + '</th>' +
                '<th class="num col-label-b" title="' + escapeAttr(b.name) + '">' + escapeHtml(labelB) + '</th>' +
                '<th class="num">\u0394</th>' +
                '</tr></thead><tbody>';
            for (var i = 0; i < durResources.length; i++) {
                var r = durResources[i];
                html += '<tr>' +
                    '<td><span class="pkg-name">' + escapeHtml(r.package) + '</span></td>' +
                    '<td class="num mono">' + (r.durA != null ? fmtDuration(r.durA) : '-') + '</td>' +
                    '<td class="num mono">' + (r.durB != null ? fmtDuration(r.durB) : '-') + '</td>' +
                    '<td class="num mono">' + fmtDelta(r.deltaDur, fmtDuration, 1) + '</td>' +
                    '</tr>';
            }
            html += '</tbody></table></div>';
        }

        // Memory table.
        var memResources = resources.filter(function(r) { return r.deltaMem != null; })
            .sort(function(x, y) { return Math.abs(y.deltaMem) - Math.abs(x.deltaMem); });

        if (memResources.length > 0) {
            html += '<div class="compare-section">' +
                '<h3>Peak Memory</h3>' +
                '<table><thead><tr>' +
                '<th>Package</th>' +
                '<th class="num col-label-a" title="' + escapeAttr(a.name) + '">' + escapeHtml(labelA) + '</th>' +
                '<th class="num col-label-b" title="' + escapeAttr(b.name) + '">' + escapeHtml(labelB) + '</th>' +
                '<th class="num">\u0394</th>' +
                '</tr></thead><tbody>';
            for (var i = 0; i < memResources.length; i++) {
                var r = memResources[i];
                html += '<tr>' +
                    '<td><span class="pkg-name">' + escapeHtml(r.package) + '</span></td>' +
                    '<td class="num mono">' + (r.memA != null ? r.memA + ' MB' : '-') + '</td>' +
                    '<td class="num mono">' + (r.memB != null ? r.memB + ' MB' : '-') + '</td>' +
                    '<td class="num mono">' + fmtDelta(r.deltaMem, function(v) { return Math.round(v) + ' MB'; }, 4) + '</td>' +
                    '</tr>';
            }
            html += '</tbody></table></div>';
        }
    }

    content.innerHTML = html;
}
