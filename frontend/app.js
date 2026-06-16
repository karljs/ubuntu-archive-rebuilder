// Rebuild Experiments Viewer

const DATA_BASE_URL = './data';
const SQL_JS_CDN = 'https://cdn.jsdelivr.net/npm/sql.js@1.12.0/dist/';

// ── Global state ──
let sqlDb = null;
let batches = [];          // all batches, enriched with .stats and .config
let sortColumn = 'package';
let sortDirection = 'asc';
let currentBatch = null;   // batch object currently shown in Details
let currentBatchData = null; // { builds, finding_summary }

// profile_configs lookup: profile_name -> { flag_summary, flags_json, has_flags }
var profileConfigMap = {};

// Compare tab state: ordered list of selected batch IDs
var compareSelectedIds = [];

// Known Ubuntu series in release order
var SERIES_ORDER = ['focal','groovy','hirsute','impish','jammy','kinetic',
                    'lunar','mantic','noble','oracular','plucky','questing',
                    'resolute','stonking'];

// ════════════════════════════════════════════════
// Bootstrap
// ════════════════════════════════════════════════

if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
} else {
    init();
}

async function init() {
    try {
        var SQL = await initSqlJs({ locateFile: function(f) { return SQL_JS_CDN + f; } });
        var buf = await fetch(DATA_BASE_URL + '/rebuild.db?v=' + Date.now()).then(function(r) {
            if (!r.ok) throw new Error('rebuild.db not found — run: rebuilder export');
            return r.arrayBuffer();
        });
        sqlDb = new SQL.Database(new Uint8Array(buf));
        el('loading-overlay').classList.add('hidden');
        loadData();
        setupEventListeners();
        // Record the initial overview state so the back button can return here.
        history.replaceState({ tab: 'overview' }, '');
        renderOverview();
    } catch(err) {
        console.error('Init failed:', err);
        el('loading-overlay').innerHTML = '<p class="load-error">Failed to load: ' + escapeHtml(String(err.message || err)) + '</p>';
    }
}

// ════════════════════════════════════════════════
// Data loading
// ════════════════════════════════════════════════

function loadData() {
    // Load profile_configs first so configFor() works during batch enrichment.
    profileConfigMap = {};
    try {
        dbQuery("SELECT id, profile_name, has_flags, flag_summary, flags_json FROM profile_configs")
            .forEach(function(r) {
                profileConfigMap[r.profile_name] = {
                    flag_summary: r.flag_summary,
                    flags_json: r.flags_json,
                    has_flags: Number(r.has_flags)
                };
            });
    } catch(e) {
        console.warn('profile_configs not found — re-run: rebuilder export');
    }

    var statRows = dbQuery(
        "SELECT batch_id, status, COUNT(*) AS count FROM builds GROUP BY batch_id, status"
    );
    var statsMap = {};
    statRows.forEach(function(r) {
        if (!statsMap[r.batch_id]) statsMap[r.batch_id] = { total:0, succeeded:0, failed:0, dep_wait:0, timeout:0 };
        var s = statsMap[r.batch_id], n = Number(r.count);
        s.total += n;
        if (r.status === 'succeeded') s.succeeded = n;
        else if (r.status === 'failed')   s.failed   = n;
        else if (r.status === 'dep_wait') s.dep_wait = n;
        else if (r.status === 'timeout')  s.timeout  = n;
    });

    batches = dbQuery(
        "SELECT id, name, compiler_type, compiler_version, series, profile_name, started_at, finished_at " +
        "FROM batches ORDER BY started_at DESC"
    ).map(function(row) {
        var b = {
            id: row.id,
            name: row.name,
            compiler_type: row.compiler_type,
            compiler_version: row.compiler_version,
            series: row.series,
            profile_name: row.profile_name,
            started_at: row.started_at,
            finished_at: row.finished_at,
            stats: statsMap[row.id] || { total:0, succeeded:0, failed:0, dep_wait:0, timeout:0 }
        };
        b.config = configFor(b);
        return b;
    });

    // Populate Details batch selector.
    populateDetailsBatchSelector();

    // Populate Compare batch list.
    renderCompareBatchList();

    // Pre-select most recent batch in Details.
    if (batches.length > 0) loadDetailsForBatch(batches[0].id, false);
}

function loadBatchData(batchId) {
    var buildRows = dbQuery(
        "SELECT id, source_package AS package, version, status, " +
        "build_duration_seconds AS duration_seconds, peak_memory_mb " +
        "FROM builds WHERE batch_id = ? ORDER BY source_package",
        [batchId]
    );
    var countMap = {};
    dbQuery(
        "SELECT build_id, COUNT(*) AS count FROM build_findings " +
        "WHERE build_id IN (SELECT id FROM builds WHERE batch_id = ?) GROUP BY build_id",
        [batchId]
    ).forEach(function(r) { countMap[r.build_id] = Number(r.count); });

    var summaryRows = dbQuery(
        "SELECT bf.category, bf.severity, COUNT(*) AS count " +
        "FROM build_findings bf JOIN builds b ON bf.build_id = b.id " +
        "WHERE b.batch_id = ? GROUP BY bf.category, bf.severity ORDER BY bf.severity, count DESC",
        [batchId]
    );
    var errors = [], observations = [];
    summaryRows.forEach(function(r) {
        var item = { category: r.category, count: Number(r.count) };
        if (r.severity === 'observation') observations.push(item);
        else errors.push(item);
    });
    return {
        builds: buildRows.map(function(row) {
            return {
                id: row.id, package: row.package, version: row.version,
                status: row.status, duration_seconds: row.duration_seconds,
                peak_memory_mb: row.peak_memory_mb, finding_count: countMap[row.id] || 0
            };
        }),
        finding_summary: errors,
        observation_summary: observations
    };
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
    // Callers that want to manage their own history entry pass pushHistory=false.
    // Plain tab-button clicks pass nothing and get a history entry here.
    if (pushHistory !== false) {
        pushView({ tab: tabName, batchId: currentBatch ? currentBatch.id : null });
    }
}

function getActiveTab() {
    var btn = document.querySelector('.tab-btn.active');
    return btn ? btn.dataset.tab : 'overview';
}

// Navigate to Details for a specific batch (called from Overview row click,
// profile comparison, version table, etc.)  One history entry total.
function navigateToDetails(batchId) {
    loadDetailsForBatch(batchId, false);  // don't push yet
    switchTab('details', false);          // don't push yet
    pushView({ tab: 'details', batchId: batchId });  // push once
}

// Navigate to Compare pre-populated with an array of batch IDs.
function navigateToCompare(batchIds) {
    compareSelectedIds = batchIds.slice();
    switchTab('compare', false);          // don't push yet
    pushView({ tab: 'compare', compareIds: batchIds });  // push once
    renderCompareBatchList();
    renderCompareTable();
}

// ════════════════════════════════════════════════
// Overview tab — success rate matrix
// ════════════════════════════════════════════════

function renderOverview() {
    var container = el('overview-matrix');
    if (!container) return;

    if (batches.length === 0) {
        container.innerHTML = '<p class="muted" style="padding:1rem">No batches found. Run: rebuilder export</p>';
        return;
    }

    // Row keys: "compiler_type compiler_version", sorted clang-first then numeric.
    var rowSet = {};
    batches.forEach(function(b) { rowSet[b.compiler_type + ' ' + b.compiler_version] = true; });
    var rows = Object.keys(rowSet).sort(function(a, b) {
        var aP = a.split(' '), bP = b.split(' ');
        var typeOrd = { clang: 0, gcc: 1 };
        var td = (typeOrd[aP[0]] !== undefined ? typeOrd[aP[0]] : 99) -
                 (typeOrd[bP[0]] !== undefined ? typeOrd[bP[0]] : 99);
        return td !== 0 ? td : parseFloat(aP[1]) - parseFloat(bP[1]);
    });

    // Column keys: unique series sorted by release order.
    var seriesSet = {};
    batches.forEach(function(b) { seriesSet[b.series] = true; });
    var cols = Object.keys(seriesSet).sort(function(a, b) {
        var ai = SERIES_ORDER.indexOf(a), bi = SERIES_ORDER.indexOf(b);
        return (ai === -1 ? 999 : ai) - (bi === -1 ? 999 : bi);
    });

    // Group batches by (compilerKey, series, profile_name) — pick largest-N per profile.
    var cellProfiles = {}; // "compilerKey\0series" -> { profile_name -> best_batch }
    batches.forEach(function(b) {
        var cellKey = b.compiler_type + ' ' + b.compiler_version + '\x00' + b.series;
        if (!cellProfiles[cellKey]) cellProfiles[cellKey] = {};
        var prev = cellProfiles[cellKey][b.profile_name];
        if (!prev || b.stats.total > prev.stats.total ||
            (b.stats.total === prev.stats.total && b.started_at > prev.started_at)) {
            cellProfiles[cellKey][b.profile_name] = b;
        }
    });

    var html = '<table class="matrix-table"><thead><tr>';
    html += '<th class="matrix-corner">Compiler</th>';
    cols.forEach(function(s) { html += '<th class="matrix-series-header">' + escapeHtml(s) + '</th>'; });
    html += '</tr></thead><tbody>';

    rows.forEach(function(rk) {
        html += '<tr><td class="matrix-row-label">' + escapeHtml(rk) + '</td>';
        cols.forEach(function(series) {
            var cellKey = rk + '\x00' + series;
            var profileMap = cellProfiles[cellKey];
            if (!profileMap || Object.keys(profileMap).length === 0) {
                html += '<td class="matrix-cell matrix-cell-empty"></td>';
                return;
            }

            // Sort profiles: baseline first, then by flag_summary.
            var profiles = Object.values(profileMap).sort(function(a, b) {
                var ca = a.config, cb = b.config;
                if (ca.has_flags !== cb.has_flags) return ca.has_flags - cb.has_flags;
                return ca.flag_summary.localeCompare(cb.flag_summary);
            });

            var rowsHtml = profiles.map(function(b) {
                var s = b.stats;
                var rate = s.total > 0 ? (s.succeeded / s.total) * 100 : 0;
                var lowN = s.total < 50;
                var colorCls = rateColorClass(rate);
                var flags = parseFlagsJson(b.config.flags_json);
                var flagDetail = flags.length === 0 ? 'No extra flags'
                    : flags.map(function(f) { return f.flag + ' — ' + f.reason; }).join('\n');
                var title = b.profile_name + '\n' + s.succeeded + '/' + s.total + ' succeeded\n' + flagDetail;

                return '<tr class="matrix-profile-row ' + colorCls + (lowN ? ' low-n' : '') + '" ' +
                       'data-action="go-details" data-id="' + escapeAttr(b.id) + '" title="' + escapeAttr(title) + '">' +
                       '<td class="mpr-label">' + escapeHtml(b.config.flag_summary) + '</td>' +
                       '<td class="mpr-rate">' + rate.toFixed(1) + '%</td>' +
                       '<td class="mpr-n">' + (lowN ? '⚠ ' : '') + 'N=' + s.total + '</td>' +
                       '</tr>';
            }).join('');

            html += '<td class="matrix-cell matrix-cell-multi">' +
                    '<table class="matrix-profile-table">' + rowsHtml + '</table>' +
                    '</td>';
        });
        html += '</tr>';
    });
    html += '</tbody></table>';

    container.innerHTML = html;
}

// ════════════════════════════════════════════════
// Details tab
// ════════════════════════════════════════════════

function populateDetailsBatchSelector() {
    var opts = batches.map(function(b) {
        var rate = b.stats.total > 0 ? Math.round(b.stats.succeeded / b.stats.total * 100) : 0;
        return {
            value: b.id,
            label: b.profile_name + ' · ' + b.series + '  (' + rate + '%, N=' + b.stats.total + ')'
        };
    });
    setDropdownOptions('details-batch-dd', opts);
}

function loadDetailsForBatch(batchId, pushHistory) {
    currentBatch = batches.find(function(b) { return b.id === batchId; });
    if (!currentBatch) return;
    currentBatchData = loadBatchData(batchId);

    // Update selector to reflect current batch.
    setDropdownValue('details-batch-dd', batchId);

    renderDetailsContext();
    renderDetailsStatusBar();
    renderDetailsFindings();
    renderBuildsTable();
    renderProfileComparison();
    renderVersionContext();

    // Only push when called directly (e.g. batch dropdown change).
    // navigateToDetails() manages its own single push and passes false.
    if (pushHistory === true) pushView({ tab: 'details', batchId: batchId });
}

function renderDetailsContext() {
    var ctx = el('details-context');
    if (!ctx || !currentBatch) return;
    var b = currentBatch;
    var flags = parseFlagsJson(b.config.flags_json);
    var flagStr = flags.length === 0 ? 'no extra flags'
        : unique(flags.map(function(f) { return f.flag; })).join(', ');
    ctx.textContent = b.compiler_type + ' ' + b.compiler_version +
        ' · ' + b.series + ' · ' + b.config.flag_summary +
        ' (' + flagStr + ') · N=' + b.stats.total;
}

function renderDetailsStatusBar() {
    var b = currentBatch;
    if (!b) return;
    var s = b.stats;
    var rate = s.total > 0 ? ((s.succeeded / s.total) * 100).toFixed(0) : 0;
    var started = new Date(b.started_at).toLocaleString();
    var totalSecs = 0;
    if (currentBatchData) {
        currentBatchData.builds.forEach(function(bld) { totalSecs += bld.duration_seconds || 0; });
    }
    var sb = el('details-status-bar');
    if (!sb) return;
    sb.innerHTML =
        '<span class="s-pass">' + s.succeeded + ' passed</span>' +
        '<span class="s-fail">' + s.failed + ' failed</span>' +
        (s.timeout  > 0 ? '<span class="s-timeout">' + s.timeout  + ' timeout</span>' : '') +
        (s.dep_wait > 0 ? '<span class="s-depwait">' + s.dep_wait + ' dep-wait</span>' : '') +
        '<span>' + s.total + ' total</span>' +
        '<span><span class="rate-bar"><span class="rate-fill" style="width:' + rate + '%"></span></span> ' + rate + '%</span>' +
        '<span>' + fmtDuration(totalSecs) + ' total build time</span>' +
        '<span class="batch-meta">' + escapeHtml(b.compiler_type + ' ' + b.compiler_version + ' · ' + b.series + ' · ' + started) + '</span>';
}

function renderDetailsFindings() {
    var fc = el('findings-content');
    if (!fc) return;
    var errors = (currentBatchData && currentBatchData.finding_summary) || [];
    var observations = (currentBatchData && currentBatchData.observation_summary) || [];
    var unanalyzed = 0;
    if (currentBatchData) {
        currentBatchData.builds.forEach(function(bld) {
            if (bld.status !== 'succeeded' && !bld.finding_count) unanalyzed++;
        });
    }
    if (errors.length === 0 && observations.length === 0 && unanalyzed === 0) {
        fc.innerHTML = '<p class="muted">No issues in this batch.</p>';
        return;
    }
    var html = '';

    // Error findings (from failed builds)
    if (errors.length > 0 || unanalyzed > 0) {
        var errorTotal = 0;
        errors.forEach(function(f) { errorTotal += f.count; });
        html += '<p class="findings-section-label findings-label-error">Errors</p>';
        errors.forEach(function(f) {
            html += '<div class="findings-bar-item">' +
                '<span class="findings-bar-count">' + f.count + '</span>' +
                '<span class="findings-bar-label" title="' + escapeAttr(f.category) + '">' + escapeHtml(f.category) + '</span>' +
                '</div>';
        });
        if (unanalyzed > 0) {
            html += '<div class="findings-bar-item findings-bar-unanalyzed">' +
                '<span class="findings-bar-count">' + unanalyzed + '</span>' +
                '<span class="findings-bar-label">Unanalyzed (no patterns matched)</span></div>';
        }
    }

    // Observation findings (from succeeded builds)
    if (observations.length > 0) {
        html += '<p class="findings-section-label findings-label-observation" style="margin-top:6px">Observations</p>';
        observations.forEach(function(f) {
            html += '<div class="findings-bar-item findings-bar-observation">' +
                '<span class="findings-bar-count">' + f.count + '</span>' +
                '<span class="findings-bar-label" title="' + escapeAttr(f.category) + '">' + escapeHtml(f.category) + '</span>' +
                '</div>';
        });
    }

    fc.innerHTML = html;
}

// ── Panel 2: Profile comparison ──

function renderProfileComparison() {
    var panel  = el('details-panel-profiles');
    var ctxEl  = el('details-profile-context');
    var tblEl  = el('details-profile-table');
    if (!panel || !currentBatch) return;

    var b = currentBatch;
    // Find all batches with same compiler_type, compiler_version, series (sibling profiles).
    var siblings = batches.filter(function(s) {
        return s.compiler_type    === b.compiler_type &&
               s.compiler_version === b.compiler_version &&
               s.series           === b.series;
    });

    // Group by profile_name, pick largest-N per profile.
    var profileMap = {};
    siblings.forEach(function(s) {
        var prev = profileMap[s.profile_name];
        if (!prev || s.stats.total > prev.stats.total ||
            (s.stats.total === prev.stats.total && s.started_at > prev.started_at)) {
            profileMap[s.profile_name] = s;
        }
    });
    var profiles = Object.values(profileMap).sort(function(a, b) {
        if (a.config.has_flags !== b.config.has_flags) return a.config.has_flags - b.config.has_flags;
        return a.config.flag_summary.localeCompare(b.config.flag_summary);
    });

    // Hide panel if only one profile (nothing to compare).
    if (profiles.length < 2) {
        panel.classList.add('hidden');
        return;
    }
    panel.classList.remove('hidden');
    if (ctxEl) ctxEl.textContent = b.compiler_type + ' ' + b.compiler_version + ' · ' + b.series;

    // Build profile summary table.
    var html = '<table><thead><tr>' +
        '<th>Profile config</th><th>Flags</th><th class="num">N</th>' +
        '<th class="num">Succeeded</th><th class="num">Failed</th><th class="num">Rate</th>' +
        '</tr></thead><tbody>';
    profiles.forEach(function(p) {
        var s = p.stats;
        var rate = s.total > 0 ? (s.succeeded / s.total * 100).toFixed(1) : '—';
        var flags = parseFlagsJson(p.config.flags_json);
        var flagCells = flags.length === 0 ? '<span class="muted">none</span>'
            : unique(flags.map(function(f) { return f.flag; })).map(function(f) {
                var reasons = flags.filter(function(x) { return x.flag === f; })
                                   .map(function(x) { return x.var + ': ' + x.reason; }).join('\n');
                return '<code title="' + escapeAttr(reasons) + '">' + escapeHtml(f) + '</code>';
            }).join(' ');
         var isCurrent = p.id === b.id;
         var rowAttrs = isCurrent
             ? ' class="details-current-row"'
             : ' class="profile-row" data-action="go-details" data-id="' + escapeAttr(p.id) + '"' +
               ' title="Open ' + escapeAttr(p.profile_name) + ' in Details"';
         html += '<tr' + rowAttrs + '>' +
            '<td>' + escapeHtml(p.config.flag_summary) + (isCurrent ? ' <span class="muted">(current)</span>' : '') + '</td>' +
            '<td>' + flagCells + '</td>' +
            '<td class="num mono">' + s.total + '</td>' +
            '<td class="num mono s-pass">' + s.succeeded + '</td>' +
            '<td class="num mono s-fail">' + s.failed + (s.timeout ? '+' + s.timeout + 't' : '') + '</td>' +
            '<td class="num mono">' + rate + '%</td>' +
            '</tr>';
    });
    html += '</tbody></table>';
    // Add Compare button for all profiles in this cell.
    var ids = profiles.map(function(p) { return p.id; });
    html += '<p style="margin-top:6px"><button class="btn btn-sm" data-action="go-compare" data-ids="' +
        escapeAttr(JSON.stringify(ids)) + '">Open in Compare \u2192</button></p>';
    if (tblEl) tblEl.innerHTML = html;
}

// ── Panel 3: Version context ──

function renderVersionContext() {
    var panel  = el('details-panel-versions');
    var ctxEl  = el('details-version-context');
    var tblEl  = el('details-version-table');
    if (!panel || !currentBatch) return;

    var b = currentBatch;
    var summary = b.config.flag_summary;

    // Find all batches with same series, same flag_summary, same compiler_type.
    // Group by compiler_version, pick largest-N per version.
    var related = batches.filter(function(s) {
        return s.series === b.series &&
               s.config.flag_summary === summary &&
               s.compiler_type === b.compiler_type;
    });

    var verMap = {};
    related.forEach(function(s) {
        var v = s.compiler_version;
        var prev = verMap[v];
        if (!prev || s.stats.total > prev.stats.total ||
            (s.stats.total === prev.stats.total && s.started_at > prev.started_at)) {
            verMap[v] = s;
        }
    });

    var versions = Object.keys(verMap).sort(function(a, b) { return parseFloat(a) - parseFloat(b); });

    if (versions.length < 2) {
        panel.classList.add('hidden');
        return;
    }
    panel.classList.remove('hidden');
    // Subtitle explains exactly what is held constant so the user knows what they are comparing.
    if (ctxEl) ctxEl.textContent =
        b.compiler_type + ' on ' + b.series + ', ' + summary + ' — success rate by version';

    var html = '<table><thead><tr>' +
        '<th>' + escapeHtml(b.compiler_type) + ' version</th>' +
        '<th class="num">N</th><th class="num">Succeeded</th><th class="num">Failed</th>' +
        '<th class="num">Rate</th>' +
        '</tr></thead><tbody>';

    versions.forEach(function(v) {
        var bv = verMap[v];
        var s = bv.stats;
        var rate = s.total > 0 ? (s.succeeded / s.total * 100) : 0;
        var lowN = s.total < 50;
        var isCurrent = bv.id === b.id;
        html += '<tr class="ver-row' + (isCurrent ? ' details-current-row' : '') + '"' +
            ' data-action="go-details" data-id="' + escapeAttr(bv.id) + '"' +
            ' title="Open ' + escapeAttr(bv.profile_name) + ' in Details">' +
            '<td class="mono">' + escapeHtml(v) + (isCurrent ? ' <span class="muted">(current)</span>' : '') + '</td>' +
            '<td class="num mono">' + (lowN ? '⚠ ' : '') + s.total + '</td>' +
            '<td class="num mono s-pass">' + s.succeeded + '</td>' +
            '<td class="num mono s-fail">' + s.failed + '</td>' +
            '<td class="num mono">' + rate.toFixed(1) + '%</td>' +
            '</tr>';
    });
    html += '</tbody></table>';
    if (tblEl) tblEl.innerHTML = html;
}

// ════════════════════════════════════════════════
// Builds table (Details Panel 1)
// ════════════════════════════════════════════════

function renderBuildsTable() {
    if (!currentBatchData) return;
    var tbody = el('builds-tbody');
    if (!tbody) return;

    var builds = currentBatchData.builds.slice();
    var filt = (el('filter-input') || {}).value;
    filt = filt ? filt.toLowerCase() : '';
    var statFilt = getDropdownValue('status-filter-dd');

    builds = builds.filter(function(b) {
        if (filt    && b.package.toLowerCase().indexOf(filt) === -1) return false;
        if (statFilt && b.status !== statFilt) return false;
        return true;
    });

    builds.sort(function(a, b) {
        var av, bv;
        switch (sortColumn) {
            case 'package':  av = a.package;              bv = b.package;              break;
            case 'status':   av = a.status;               bv = b.status;               break;
            case 'duration': av = a.duration_seconds || 0; bv = b.duration_seconds || 0; break;
            case 'memory':   av = a.peak_memory_mb    || 0; bv = b.peak_memory_mb    || 0; break;
            case 'findings': av = a.finding_count     || 0; bv = b.finding_count     || 0; break;
            default:         av = a.package;              bv = b.package;
        }
        if (typeof av === 'string') return sortDirection === 'asc' ? av.localeCompare(bv) : bv.localeCompare(av);
        return sortDirection === 'asc' ? av - bv : bv - av;
    });

    document.querySelectorAll('th[data-sort]').forEach(function(th) {
        th.classList.remove('sort-asc','sort-desc');
        if (th.dataset.sort === sortColumn) th.classList.add(sortDirection === 'asc' ? 'sort-asc' : 'sort-desc');
    });

    var html = '';
    builds.forEach(function(b) {
        var issues = b.finding_count > 0 ? String(b.finding_count)
            : b.status === 'succeeded' ? '<span class="cell-hint" data-hint="No issues detected">0</span>'
            : (b.status === 'failed' || b.status === 'timeout' || b.status === 'dep_wait')
              ? '<span class="cell-hint" data-hint="Build did not complete">n/a</span>' : '-';
        html += '<tr>' +
            '<td><span class="pkg-name">' + escapeHtml(b.package) + '</span></td>' +
            '<td><span class="st st-' + b.status + '">' + b.status + '</span></td>' +
            '<td class="num mono">' + (b.duration_seconds ? fmtDuration(b.duration_seconds) : '-') + '</td>' +
            '<td class="num mono">' + (b.peak_memory_mb ? b.peak_memory_mb + ' MB' : '-') + '</td>' +
            '<td class="num">' + issues + '</td>' +
            '<td>' +
                (b.finding_count > 0 ? '<button class="btn-link" data-action="issues" data-id="' + b.id + '">issues</button> ' : '') +
                '<button class="btn-link" data-action="log" data-id="' + b.id + '" data-pkg="' + escapeAttr(b.package) + '">log</button>' +
            '</td></tr>';
    });
    tbody.innerHTML = html;
}

function handleSort(col) {
    sortDirection = (sortColumn === col && sortDirection === 'asc') ? 'desc' : 'asc';
    sortColumn = col;
    renderBuildsTable();
}

// ════════════════════════════════════════════════
// Compare tab — N-way batch comparison
// ════════════════════════════════════════════════

function renderCompareBatchList() {
    var list = el('compare-batch-list');
    if (!list) return;
    var filter = (el('compare-filter-input') || {}).value || '';
    filter = filter.toLowerCase();

    var html = '';
    batches.forEach(function(b) {
        var label = b.profile_name + ' · ' + b.series;
        if (filter && label.toLowerCase().indexOf(filter) === -1) return;
        var rate = b.stats.total > 0 ? Math.round(b.stats.succeeded / b.stats.total * 100) : 0;
        var checked = compareSelectedIds.indexOf(b.id) !== -1;
        html += '<li>' +
            '<label class="compare-check-label">' +
            '<input type="checkbox" class="compare-batch-cb" value="' + escapeAttr(b.id) + '"' + (checked ? ' checked' : '') + '>' +
            '<span class="compare-batch-name">' + escapeHtml(label) + '</span>' +
            '<span class="compare-rate-bar ' + rateColorClass(rate) + '"></span>' +
            '<span class="compare-rate-num">' + rate + '%</span>' +
            '</label></li>';
    });
    list.innerHTML = html || '<li class="muted" style="padding:4px 8px">No batches match filter</li>';

    list.querySelectorAll('.compare-batch-cb').forEach(function(cb) {
        cb.addEventListener('change', function() {
            var id = this.value;
            if (this.checked) {
                if (compareSelectedIds.indexOf(id) === -1) compareSelectedIds.push(id);
            } else {
                compareSelectedIds = compareSelectedIds.filter(function(x) { return x !== id; });
            }
            renderCompareTable();
        });
    });
}

function renderCompareTable() {
    var content = el('compare-content');
    if (!content) return;

    if (compareSelectedIds.length < 2) {
        content.innerHTML = '<p class="muted compare-hint">Select 2 or more batches from the list to compare.</p>';
        return;
    }

    // Load build data for each selected batch.
    var selectedBatches = compareSelectedIds.map(function(id) {
        return batches.find(function(b) { return b.id === id; });
    }).filter(Boolean);

    var batchData = selectedBatches.map(function(b) {
        var builds = dbQuery(
            "SELECT source_package, status, build_duration_seconds AS dur, peak_memory_mb AS mem, id " +
            "FROM builds WHERE batch_id = ?", [b.id]
        );
        // Top category per failing build.
        var cats = {};
        dbQuery(
            "SELECT b.source_package, bf.category, COUNT(*) as cnt " +
            "FROM build_findings bf JOIN builds b ON bf.build_id = b.id " +
            "WHERE b.batch_id = ? GROUP BY b.source_package, bf.category",
            [b.id]
        ).forEach(function(r) {
            if (!cats[r.source_package] || r.cnt > cats[r.source_package].cnt)
                cats[r.source_package] = { category: r.category, cnt: Number(r.cnt) };
        });
        var map = {};
        builds.forEach(function(bld) { map[bld.source_package] = bld; });
        return { batch: b, map: map, cats: cats };
    });

    // Union of all packages across selected batches.
    var pkgSet = {};
    batchData.forEach(function(d) { Object.keys(d.map).forEach(function(p) { pkgSet[p] = true; }); });
    var allPkgs = Object.keys(pkgSet).sort();

    // Classify each package: has any failure across any batch?
    var mixed = [], allFail = [], allSucc = [];
    allPkgs.forEach(function(pkg) {
        var statuses = batchData.map(function(d) { return d.map[pkg] ? d.map[pkg].status : null; });
        var anyFail = statuses.some(function(s) { return s && s !== 'succeeded'; });
        var anySucc = statuses.some(function(s) { return s === 'succeeded'; });
        var anyMissing = statuses.some(function(s) { return s === null; });
        if (anyMissing || (anyFail && anySucc)) mixed.push(pkg);
        else if (anyFail) allFail.push(pkg);
        else allSucc.push(pkg);
    });

    // Column headers.
    var colW = Math.max(80, Math.floor(600 / selectedBatches.length));
    var headerHtml = '<th>Package</th>';
    selectedBatches.forEach(function(b) {
        var label = b.profile_name + '<br><span class="compare-col-series">' + escapeHtml(b.series) + ' · ' + escapeHtml(b.config.flag_summary) + '</span>';
        headerHtml += '<th class="compare-col-batch" style="min-width:' + colW + 'px" title="' + escapeAttr(b.profile_name + ' · ' + b.series) + '">' + label + '</th>';
    });
    headerHtml += '<th class="actions-col">Log</th>';

    function pkgRow(pkg) {
        var cells = batchData.map(function(d) {
            var bld = d.map[pkg];
            if (!bld) return '<td class="compare-cell compare-cell-missing"><span class="muted">—</span></td>';
            var st = bld.status;
            var cat = (st !== 'succeeded' && d.cats[pkg]) ? d.cats[pkg].category : null;
            var cls = st === 'succeeded' ? 'compare-cell-ok' : st === 'failed' ? 'compare-cell-fail' : 'compare-cell-other';
            return '<td class="compare-cell ' + cls + '">' +
                '<span class="st st-' + st + '">' + st + '</span>' +
                (cat ? '<br><span class="compare-cat">' + escapeHtml(cat) + '</span>' : '') +
                '</td>';
        });

        // Log link: first batch where this package failed.
        var logCell = '<td>';
        for (var i = 0; i < batchData.length; i++) {
            var bld = batchData[i].map[pkg];
            if (bld && bld.status !== 'succeeded') {
                logCell += '<button class="btn-link" data-action="log" data-id="' + escapeAttr(bld.id) + '" data-pkg="' + escapeAttr(pkg) + '">log</button>';
                break;
            }
        }
        logCell += '</td>';

        return '<tr><td><span class="pkg-name">' + escapeHtml(pkg) + '</span></td>' + cells.join('') + logCell + '</tr>';
    }

    var html = '<table class="compare-table"><thead><tr>' + headerHtml + '</tr></thead><tbody>';

    // Mixed outcome / partially failing packages first.
    mixed.forEach(function(pkg) { html += pkgRow(pkg); });

    // All-failing packages.
    if (allFail.length > 0) {
        html += '<tr class="compare-section-row"><td colspan="' + (selectedBatches.length + 2) + '">Failing in all selected batches (' + allFail.length + ')</td></tr>';
        allFail.forEach(function(pkg) { html += pkgRow(pkg); });
    }

    // All-succeeded — collapsed.
    if (allSucc.length > 0) {
        html += '<tr class="compare-section-row compare-section-collapsed" data-toggle="compare-succ">' +
            '<td colspan="' + (selectedBatches.length + 2) + '">' + allSucc.length + ' succeeded in all — click to expand</td></tr>';
        html += '<tbody id="compare-succ-rows" class="hidden">';
        allSucc.forEach(function(pkg) { html += pkgRow(pkg); });
        html += '</tbody>';
    }

    html += '</tbody></table>';

    // Resource comparison — available for any number of batches.
    html += renderResourceComparison(batchData, selectedBatches);

    content.innerHTML = html;

    // Wire expand toggle.
    var tog = content.querySelector('[data-toggle="compare-succ"]');
    if (tog) {
        tog.addEventListener('click', function() {
            var body = el('compare-succ-rows');
            if (body) {
                body.classList.toggle('hidden');
                this.classList.toggle('compare-section-collapsed');
            }
        });
    }
}

function renderResourceComparison(batchData, selectedBatches) {
    var n = selectedBatches.length;
    var pairwise = n === 2;

    // Build column headers — short label per batch.
    var colHeaders = selectedBatches.map(function(b) {
        return escapeHtml(b.profile_name + ' · ' + b.series);
    });

    // Collect all packages that have resource data in at least one batch.
    var pkgSet = {};
    batchData.forEach(function(d) {
        Object.keys(d.map).forEach(function(pkg) {
            var bld = d.map[pkg];
            if (bld && (bld.dur != null || bld.mem != null)) pkgSet[pkg] = true;
        });
    });
    var allPkgs = Object.keys(pkgSet).sort();
    if (allPkgs.length === 0) return '';

    var html = '';

    // Build Time table.
    // Sort by: largest spread between max and min duration across batches.
    var durRows = allPkgs.map(function(pkg) {
        var vals = batchData.map(function(d) {
            var bld = d.map[pkg];
            return (bld && bld.dur != null) ? bld.dur : null;
        });
        var defined = vals.filter(function(v) { return v != null; });
        if (defined.length === 0) return null;
        var spread = defined.length > 1
            ? Math.max.apply(null, defined) - Math.min.apply(null, defined) : 0;
        return { pkg: pkg, vals: vals, spread: spread };
    }).filter(Boolean).sort(function(a, b) { return b.spread - a.spread; });

    if (durRows.length > 0) {
        html += '<div class="compare-section"><h3>Build Time</h3><table><thead><tr><th>Package</th>';
        colHeaders.forEach(function(h) { html += '<th class="num">' + h + '</th>'; });
        if (pairwise) html += '<th class="num">\u0394</th>';
        html += '</tr></thead><tbody>';
        durRows.forEach(function(r) {
            html += '<tr><td><span class="pkg-name">' + escapeHtml(r.pkg) + '</span></td>';
            r.vals.forEach(function(v) {
                html += '<td class="num mono">' + (v != null ? fmtDuration(v) : '-') + '</td>';
            });
            if (pairwise && r.vals[0] != null && r.vals[1] != null) {
                html += '<td class="num mono">' + fmtDelta(r.vals[1] - r.vals[0], fmtDuration, 1) + '</td>';
            } else if (pairwise) {
                html += '<td class="num mono muted">-</td>';
            }
            html += '</tr>';
        });
        html += '</tbody></table></div>';
    }

    // Peak Memory table.
    var memRows = allPkgs.map(function(pkg) {
        var vals = batchData.map(function(d) {
            var bld = d.map[pkg];
            return (bld && bld.mem != null) ? bld.mem : null;
        });
        var defined = vals.filter(function(v) { return v != null; });
        if (defined.length === 0) return null;
        var spread = defined.length > 1
            ? Math.max.apply(null, defined) - Math.min.apply(null, defined) : 0;
        return { pkg: pkg, vals: vals, spread: spread };
    }).filter(Boolean).sort(function(a, b) { return b.spread - a.spread; });

    if (memRows.length > 0) {
        html += '<div class="compare-section"><h3>Peak Memory</h3><table><thead><tr><th>Package</th>';
        colHeaders.forEach(function(h) { html += '<th class="num">' + h + '</th>'; });
        if (pairwise) html += '<th class="num">\u0394</th>';
        html += '</tr></thead><tbody>';
        memRows.forEach(function(r) {
            html += '<tr><td><span class="pkg-name">' + escapeHtml(r.pkg) + '</span></td>';
            r.vals.forEach(function(v) {
                html += '<td class="num mono">' + (v != null ? v + ' MB' : '-') + '</td>';
            });
            if (pairwise && r.vals[0] != null && r.vals[1] != null) {
                html += '<td class="num mono">' + fmtDelta(r.vals[1] - r.vals[0], function(v) { return Math.round(v) + ' MB'; }, 4) + '</td>';
            } else if (pairwise) {
                html += '<td class="num mono muted">-</td>';
            }
            html += '</tr>';
        });
        html += '</tbody></table></div>';
    }

    return html;
}

// ════════════════════════════════════════════════
// Event listeners
// ════════════════════════════════════════════════

function setupEventListeners() {
    document.querySelectorAll('.tab-btn').forEach(function(btn) {
        btn.addEventListener('click', function() { switchTab(this.dataset.tab); });
    });

    initDropdown('details-batch-dd', function(val) { loadDetailsForBatch(val, true); });
    initDropdown('status-filter-dd', function() { renderBuildsTable(); });

    var cfi = el('compare-filter-input');
    if (cfi) cfi.addEventListener('input', renderCompareBatchList);

    var fi = el('filter-input');
    if (fi) fi.addEventListener('input', renderBuildsTable);

    document.querySelectorAll('th[data-sort]').forEach(function(th) {
        th.addEventListener('click', function() { handleSort(this.dataset.sort); });
    });

    var mc = el('modal-close');
    if (mc) mc.addEventListener('click', closeModal);
    var lmc = el('log-modal-close');
    if (lmc) lmc.addEventListener('click', closeLogModal);

    var m = el('modal');
    if (m) m.addEventListener('click', function(e) { if (e.target === this) closeModal(); });
    var lm = el('log-modal');
    if (lm) lm.addEventListener('click', function(e) { if (e.target === this) closeLogModal(); });

    var ls = el('log-search');
    if (ls) ls.addEventListener('input', handleLogSearch);

    document.addEventListener('keydown', function(e) {
        if (e.key === 'Escape') {
            if (el('log-modal') && !el('log-modal').classList.contains('hidden')) { closeLogModal(); return; }
            if (el('modal') && !el('modal').classList.contains('hidden')) { closeModal(); return; }
        }
    });
}

// Delegated handler for log/details buttons anywhere in the document.
document.addEventListener('click', function(e) {
    if (e.target.closest('.dropdown')) return;
    var btn = e.target.closest('[data-action]');
    if (!btn) return;
    var action = btn.getAttribute('data-action');
    var id = btn.getAttribute('data-id');
    if (action === 'details') showBuildDetails(id);
    if (action === 'issues')  showBuildDetails(id);
    if (action === 'log') showBuildLog(id, btn.getAttribute('data-pkg'));
    if (action === 'go-details') navigateToDetails(id);
    if (action === 'go-compare') {
        var ids = JSON.parse(btn.getAttribute('data-ids') || '[]');
        navigateToCompare(ids);
    }
});

// ════════════════════════════════════════════════
// Modals
// ════════════════════════════════════════════════

function closeModal()    { var m = el('modal');     if (m) m.classList.add('hidden'); document.body.style.overflow = ''; }
function closeLogModal() { var m = el('log-modal'); if (m) m.classList.add('hidden'); document.body.style.overflow = ''; }

function showBuildDetails(buildId) {
    var findings = dbQuery(
        "SELECT category, description, excerpt, line_number, severity " +
        "FROM build_findings WHERE build_id = ? ORDER BY severity, line_number",
        [buildId]
    );
    var pkg = '';
    if (currentBatchData) {
        var bld = currentBatchData.builds.find(function(b) { return b.id === buildId; });
        if (bld) pkg = bld.package;
    }
    var mt = el('modal-title'), mb = el('modal-body');
    if (mt) mt.textContent = pkg + ' — Findings';
    if (findings.length === 0) {
        if (mb) mb.innerHTML = '<p class="muted">No findings.</p>';
    } else {
        var errors = findings.filter(function(f) { return f.severity !== 'observation'; });
        var obs    = findings.filter(function(f) { return f.severity === 'observation'; });
        var html = '';
        if (errors.length > 0) {
            html += errors.map(function(f) {
                return '<div class="finding-detail finding-detail-error">' +
                    '<h4>' + escapeHtml(f.category) + '</h4>' +
                    '<p>' + escapeHtml(f.description) + '</p>' +
                    (f.line_number ? '<p class="muted">Line ' + f.line_number + '</p>' : '') +
                    (f.excerpt ? '<pre>' + escapeHtml(f.excerpt) + '</pre>' : '') +
                    '</div>';
            }).join('');
        }
        if (obs.length > 0) {
            html += '<h4 class="finding-group-label">Observations</h4>';
            html += obs.map(function(f) {
                return '<div class="finding-detail finding-detail-observation">' +
                    '<h4>' + escapeHtml(f.category) + '</h4>' +
                    '<p>' + escapeHtml(f.description) + '</p>' +
                    (f.line_number ? '<p class="muted">Line ' + f.line_number + '</p>' : '') +
                    (f.excerpt ? '<pre>' + escapeHtml(f.excerpt) + '</pre>' : '') +
                    '</div>';
            }).join('');
        }
        if (mb) mb.innerHTML = html;
    }
    var m = el('modal');
    if (m) { m.classList.remove('hidden'); document.body.style.overflow = 'hidden'; }
}

var currentLogText = '';

async function showBuildLog(buildId, packageName) {
    var lt = el('log-modal-title');
    var lm = el('log-modal');
    var ls = el('log-search');
    var lsc = el('log-search-count');
    var lc = el('log-content');

    // Show the modal immediately so the user gets feedback.
    if (lt) lt.textContent = packageName + ' — Build Log';
    if (ls) ls.value = '';
    if (lsc) lsc.textContent = '';
    if (lc) lc.innerHTML = '<div class="log-loading">Loading\u2026</div>';
    if (lm) { lm.classList.remove('hidden'); document.body.style.overflow = 'hidden'; }

    try {
        var r = await fetch(DATA_BASE_URL + '/logs/' + buildId + '.log');
        if (r.status === 404) {
            if (lc) lc.innerHTML =
                '<div class="log-unavailable">' +
                '<p>Log not available.</p>' +
                '<p>This viewer is running without build logs. Logs are only present ' +
                'when the viewer data was exported from the machine that ran the builds.</p>' +
                '</div>';
            return;
        }
        if (!r.ok) throw new Error('HTTP ' + r.status);
        currentLogText = await r.text();
        renderLog(currentLogText);
        setTimeout(function() { if (ls) ls.focus(); }, 100);
    } catch(err) {
        if (lc) lc.innerHTML =
            '<div class="log-unavailable">' +
            '<p>Failed to load log: ' + escapeHtml(String(err.message || err)) + '</p>' +
            '</div>';
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
        var num = String(i + 1).padStart(numWidth);
        var content = escapeHtml(lines[i]);
        if (searchTerm) {
            var re = new RegExp(escapeRegex(escapeHtml(searchTerm)), 'gi');
            content = content.replace(re, function(m) { hitCount++; return '<span class="search-hit">' + m + '</span>'; });
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
    renderLog(currentLogText, ls && ls.value.trim() || null);
}

// ════════════════════════════════════════════════
// Browser history
// ════════════════════════════════════════════════

var _historyInitialised = true; // replaceState is called at init; all subsequent calls are pushState
function pushView(state) {
    history.pushState(state, '');
}
window.addEventListener('popstate', function(e) {
    if (!e.state) return;
    var tab = e.state.tab || 'overview';
    switchTab(tab, false);
    if (tab === 'details' && e.state.batchId) {
        loadDetailsForBatch(e.state.batchId, false);
    }
    if (tab === 'compare' && e.state.compareIds) {
        compareSelectedIds = e.state.compareIds.slice();
        renderCompareBatchList();
        renderCompareTable();
    }
});

// ════════════════════════════════════════════════
// Utilities
// ════════════════════════════════════════════════

function el(id) { return document.getElementById(id); }

function dbQuery(sql, params) {
    var stmt = sqlDb.prepare(sql);
    if (params) stmt.bind(params);
    var rows = [];
    while (stmt.step()) rows.push(stmt.getAsObject());
    stmt.free();
    return rows;
}

function unique(arr) { return arr.filter(function(v, i, a) { return a.indexOf(v) === i; }); }

function configFor(batch) {
    return profileConfigMap[batch.profile_name] ||
           { flag_summary: batch.profile_name, flags_json: '[]', has_flags: 0 };
}

function parseFlagsJson(json) {
    try { return JSON.parse(json) || []; } catch(e) { return []; }
}

function rateColorClass(rate) {
    if (rate >= 95) return 'rate-green';
    if (rate >= 80) return 'rate-lime';
    if (rate >= 50) return 'rate-yellow';
    if (rate >= 20) return 'rate-orange';
    return 'rate-red';
}

function fmtDuration(s) {
    if (s < 60) return Math.round(s) + 's';
    var m = Math.floor(s / 60), sec = Math.round(s % 60);
    if (m < 60) return m + 'm' + (sec > 0 ? sec + 's' : '');
    return Math.floor(m / 60) + 'h' + (m % 60 > 0 ? (m % 60) + 'm' : '');
}

function escapeHtml(text) {
    var d = document.createElement('div');
    d.textContent = String(text);
    return d.innerHTML;
}

function escapeAttr(text) {
    return String(text).replace(/&/g,'&amp;').replace(/"/g,'&quot;').replace(/'/g,'&#39;').replace(/</g,'&lt;');
}

function escapeRegex(s) { return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'); }

function fmtDelta(delta, fmt, threshold) {
    if (delta == null) return '<span class="delta-same">-</span>';
    var abs = Math.abs(delta);
    if (abs < threshold) return '<span class="delta-same">±' + fmt(abs) + '</span>';
    if (delta > 0) return '<span class="delta-worse">+' + fmt(abs) + '</span>';
    return '<span class="delta-better">−' + fmt(abs) + '</span>';
}

// ── Dropdown helpers ──

function initDropdown(containerId, onChange) {
    var dd = el(containerId);
    if (!dd) return;
    var toggle = dd.querySelector('.dropdown-toggle');
    var menu   = dd.querySelector('.dropdown-menu');
    toggle.addEventListener('click', function(e) {
        e.stopPropagation();
        document.querySelectorAll('.dropdown.open').forEach(function(d) { if (d !== dd) d.classList.remove('open'); });
        dd.classList.toggle('open');
    });
    menu.addEventListener('click', function(e) {
        var li = e.target.closest('li');
        if (!li) return;
        e.stopPropagation();
        var val = li.getAttribute('data-value');
        toggle.textContent = li.textContent;
        dd.dataset.value = val;
        menu.querySelectorAll('li').forEach(function(item) { item.classList.toggle('selected', item === li); });
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
    if (options.length > 0) { toggle.textContent = options[0].label; dd.dataset.value = options[0].value; }
}

function setDropdownValue(containerId, value) {
    var dd = el(containerId);
    if (!dd) return;
    var menu = dd.querySelector('.dropdown-menu');
    var toggle = dd.querySelector('.dropdown-toggle');
    var li = menu ? menu.querySelector('li[data-value="' + escapeAttr(value) + '"]') : null;
    if (li) {
        toggle.textContent = li.textContent;
        dd.dataset.value = value;
        menu.querySelectorAll('li').forEach(function(item) { item.classList.toggle('selected', item === li); });
    }
}

function getDropdownValue(containerId) {
    var dd = el(containerId);
    return dd ? (dd.dataset.value || '') : '';
}

document.addEventListener('click', function() {
    document.querySelectorAll('.dropdown.open').forEach(function(d) { d.classList.remove('open'); });
});
