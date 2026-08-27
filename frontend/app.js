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
let categoryFilter = null;  // active Issue-Category filter on the Details builds table

// profile_configs lookup: profile_name -> { flag_summary, flags_json, has_flags }
var profileConfigMap = {};

// Compare tab state: ordered list of selected batch IDs
var compareSelectedIds = [];

// Series release order, from export_meta.series_order (written by the
// backend from distro-info). Empty on legacy exports: first-seen fallback.
var seriesOrder = [];

// True when batches span more than one arch; arch then appears in labels.
var multiArch = false;

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

// Export schema capabilities; old exports lack columns/tables added later.
var exportHas = { attemptNumber: false, component: false, arch: false, exportMeta: false };
var STATUS_ORDER = ['succeeded', 'failed', 'oom_killed', 'timeout', 'dep_wait', 'environmental'];

function probeExportSchema() {
    try {
        exportHas.attemptNumber = dbQuery(
            "SELECT COUNT(*) AS n FROM pragma_table_info('builds') WHERE name = 'attempt_number'"
        )[0].n > 0;
        exportHas.component = dbQuery(
            "SELECT COUNT(*) AS n FROM pragma_table_info('builds') WHERE name = 'component'"
        )[0].n > 0;
        exportHas.arch = dbQuery(
            "SELECT COUNT(*) AS n FROM pragma_table_info('batches') WHERE name = 'arch'"
        )[0].n > 0;
        exportHas.exportMeta = dbQuery(
            "SELECT COUNT(*) AS n FROM pragma_table_info('export_meta')"
        )[0].n > 0;
        if (exportHas.exportMeta) {
            var row = dbQuery("SELECT value FROM export_meta WHERE key = 'series_order'")[0];
            if (row) {
                var parsed = JSON.parse(row.value);
                if (Array.isArray(parsed)) seriesOrder = parsed;
            }
        }
    } catch (e) {
        console.warn('schema probe failed, assuming legacy export:', e);
    }
}

// Matches the backend's get_batch_stats: one row per package, final attempt
// only. OOM retries write multiple rows per package.
var FINAL_ATTEMPT_WHERE = "b.attempt_number = (" +
    "SELECT MAX(b2.attempt_number) FROM builds b2 " +
    "WHERE b2.batch_id = b.batch_id AND b2.source_package = b.source_package)";

function loadData() {
    probeExportSchema();

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
        console.warn('profile_configs not found; re-run: rebuilder export');
    }

    var attemptFilter = exportHas.attemptNumber ? " WHERE " + FINAL_ATTEMPT_WHERE : "";
    var statsMap = {};
    dbQuery(
        "SELECT b.batch_id AS batch_id, b.status AS status, COUNT(*) AS count " +
        "FROM builds b" + attemptFilter + " GROUP BY b.batch_id, b.status"
    ).forEach(function(r) {
        var s = statsMap[r.batch_id] = statsMap[r.batch_id] || { total: 0, by_status: {}, environmental: 0, retried: 0 };
        var n = Number(r.count);
        s.total += n;
        s.by_status[r.status] = n;
    });

    // Environmental-only failures: failed builds whose findings are ALL
    // environmental (infra artifacts). Split out of `failed` and excluded
    // from success-rate denominators, matching the backend.
    try {
        var envWhere = "b.status = 'failed' AND " +
            "EXISTS (SELECT 1 FROM build_findings f WHERE f.build_id = b.id) AND " +
            "NOT EXISTS (SELECT 1 FROM build_findings f WHERE f.build_id = b.id AND f.finding_class <> 'environmental')";
        if (exportHas.attemptNumber) envWhere += " AND " + FINAL_ATTEMPT_WHERE;
        dbQuery(
            "SELECT b.batch_id AS batch_id, COUNT(*) AS count FROM builds b WHERE " + envWhere + " GROUP BY b.batch_id"
        ).forEach(function(r) {
            var s = statsMap[r.batch_id];
            if (!s) return;
            var n = Number(r.count);
            s.environmental = n;
            s.by_status.failed = Math.max(0, (s.by_status.failed || 0) - n);
        });
    } catch(e) {
        console.warn('finding_class not present; re-run: rebuilder export');
    }

    // Packages with >1 attempt (any batch).
    if (exportHas.attemptNumber) {
        try {
            dbQuery(
                "SELECT batch_id, COUNT(*) AS n FROM (" +
                "  SELECT batch_id, source_package FROM builds " +
                "  GROUP BY batch_id, source_package HAVING COUNT(*) > 1" +
                ") GROUP BY batch_id"
            ).forEach(function(r) {
                var s = statsMap[r.batch_id];
                if (s) s.retried = Number(r.n);
            });
        } catch(e) { /* retried stays 0 */ }
    }

    var batchCols = "id, name, compiler_type, compiler_version, series, profile_name, started_at, finished_at";
    if (exportHas.arch) batchCols += ", arch";
    batches = dbQuery(
        "SELECT " + batchCols + " FROM batches ORDER BY started_at DESC"
    ).map(function(row) {
        var b = {
            id: row.id,
            name: row.name,
            compiler_type: row.compiler_type,
            compiler_version: row.compiler_version,
            series: row.series,
            arch: row.arch || null,
            profile_name: row.profile_name,
            started_at: row.started_at,
            finished_at: row.finished_at,
            stats: statsMap[row.id] || { total: 0, by_status: {}, environmental: 0, retried: 0 }
        };
        b.config = configFor(b);
        return b;
    });

    if (seriesOrder.length === 0) {
        // First-seen order (earliest batch started_at per series), oldest left.
        var firstSeen = {};
        batches.forEach(function(b) {
            if (!firstSeen[b.series] || b.started_at < firstSeen[b.series]) firstSeen[b.series] = b.started_at;
        });
        seriesOrder = Object.keys(firstSeen).sort(function(a, b) { return firstSeen[a] < firstSeen[b] ? -1 : 1; });
    }

    var archSet = {};
    batches.forEach(function(b) { archSet[b.arch || ''] = true; });
    multiArch = Object.keys(archSet).length > 1;

    populateDetailsBatchSelector();
    populateStatusFilter();
    renderCompareBatchList();

    if (batches.length > 0) loadDetailsForBatch(batches[0].id, false);
}

function loadBatchData(batchId) {
    // Legacy exports lack attempt_number/jobs/component; probe before selecting.
    var cols = "b.id, b.source_package AS package, b.version, b.status, " +
        "b.build_duration_seconds AS duration_seconds, b.peak_memory_mb";
    if (exportHas.attemptNumber) cols += ", b.attempt_number, b.jobs";
    if (exportHas.component) cols += ", b.component";
    var finalWhere = exportHas.attemptNumber ? " AND " + FINAL_ATTEMPT_WHERE : "";
    var buildRows = dbQuery(
        "SELECT " + cols + " FROM builds b WHERE b.batch_id = ?" + finalWhere + " ORDER BY b.source_package",
        [batchId]
    );

    // Full attempt history for retried packages (all attempts, in order).
    var attemptRows = [];
    if (exportHas.attemptNumber) {
        attemptRows = dbQuery(
            "SELECT source_package, attempt_number, status, jobs FROM builds " +
            "WHERE batch_id = ? AND source_package IN (" +
            "  SELECT source_package FROM builds WHERE batch_id = ? " +
            "  GROUP BY source_package HAVING COUNT(*) > 1" +
            ") ORDER BY source_package, attempt_number",
            [batchId, batchId]
        );
    }
    var retriedPackages = {};
    attemptRows.forEach(function(r) {
        if (!retriedPackages[r.source_package]) retriedPackages[r.source_package] = [];
        retriedPackages[r.source_package].push(r);
    });

    var findingMap = {};
    dbQuery(
        "SELECT bf.build_id, bf.category, bf.finding_class, bf.severity " +
        "FROM build_findings bf JOIN builds b ON bf.build_id = b.id " +
        "WHERE b.batch_id = ?",
        [batchId]
    ).forEach(function(r) {
        var m = findingMap[r.build_id];
        if (!m) { m = findingMap[r.build_id] = { categories: {}, classes: {}, count: 0 }; }
        m.categories[r.category] = true;
        m.classes[r.finding_class] = true;
        m.count++;
    });

    var summaryRows = dbQuery(
        "SELECT bf.category, bf.severity, bf.finding_class, COUNT(DISTINCT bf.build_id) AS count " +
        "FROM build_findings bf JOIN builds b ON bf.build_id = b.id " +
        "WHERE b.batch_id = ? GROUP BY bf.category, bf.severity, bf.finding_class ORDER BY bf.severity, count DESC",
        [batchId]
    );
    var errors = [], observations = [];
    summaryRows.forEach(function(r) {
        var item = { category: r.category, count: Number(r.count), finding_class: r.finding_class };
        if (r.severity === 'observation') observations.push(item);
        else errors.push(item);
    });
    return {
        builds: buildRows.map(function(row) {
            var m = findingMap[row.id];
            var categories = m ? Object.keys(m.categories) : [];
            var classes = m ? Object.keys(m.classes) : [];
            var envOnly = classes.length > 0 && classes.every(function(c) { return c === 'environmental'; });
            var retries = retriedPackages[row.package] || [];
            return {
                id: row.id, package: row.package, version: row.version,
                status: row.status, duration_seconds: row.duration_seconds,
                peak_memory_mb: row.peak_memory_mb,
                component: row.component || null,
                attempt_number: row.attempt_number || 1,
                jobs: row.jobs,
                retries: retries,
                finding_count: m ? m.count : 0,
                categories: categories,
                env_only: envOnly
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

    // Row keys: compiler_type alphabetically, then natural version order
    // ("9" < "10" < "2.28"). No compiler list is hardcoded.
    var rowSet = {};
    batches.forEach(function(b) { rowSet[b.compiler_type + ' ' + b.compiler_version] = true; });
    var rows = Object.keys(rowSet).sort(function(a, b) {
        var aP = a.split(' '), bP = b.split(' ');
        var td = aP[0].localeCompare(bP[0]);
        return td !== 0 ? td : compareVersions(aP[1], bP[1]);
    });

    // Columns: one per series, or per (series, arch) when the data spans
    // multiple arches. Ordered by series release order from export_meta.
    var colMap = {};
    batches.forEach(function(b) {
        var ck = b.series + (multiArch ? '\x01' + (b.arch || '') : '');
        if (!colMap[ck]) colMap[ck] = true;
    });
    var cols = Object.keys(colMap).sort(function(x, y) {
        var xs = x.split('\x01')[0], ys = y.split('\x01')[0];
        var xi = seriesOrder.indexOf(xs), yi = seriesOrder.indexOf(ys);
        var d = (xi === -1 ? seriesOrder.length : xi) - (yi === -1 ? seriesOrder.length : yi);
        return d !== 0 ? d : x.localeCompare(y);
    });

    // Group batches by (compilerKey, column, profile_name); largest-N per profile.
    var cellProfiles = {};
    batches.forEach(function(b) {
        var colKey = b.series + (multiArch ? '\x01' + (b.arch || '') : '');
        var cellKey = b.compiler_type + ' ' + b.compiler_version + '\x00' + colKey;
        if (!cellProfiles[cellKey]) cellProfiles[cellKey] = {};
        var prev = cellProfiles[cellKey][b.profile_name];
        if (!prev || b.stats.total > prev.stats.total ||
            (b.stats.total === prev.stats.total && b.started_at > prev.started_at)) {
            cellProfiles[cellKey][b.profile_name] = b;
        }
    });

    var html = '<table class="matrix-table"><thead><tr>';
    html += '<th class="matrix-corner">Compiler</th>';
    cols.forEach(function(ck) {
        var parts = ck.split('\x01');
        var label = parts[0] + (parts[1] !== undefined ? ' · ' + (parts[1] || '?') : '');
        html += '<th class="matrix-series-header">' + escapeHtml(label) + '</th>';
    });
    html += '</tr></thead><tbody>';

    rows.forEach(function(rk) {
        html += '<tr><td class="matrix-row-label">' + escapeHtml(rk) + '</td>';
        cols.forEach(function(ck) {
            var cellKey = rk + '\x00' + ck;
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
                var rate = successRate(s);
                var lowN = comparableTotal(s) < 50;
                var colorCls = rateColorClass(rate);
                var flags = parseFlagsJson(b.config.flags_json);
                var flagDetail = flags.length === 0 ? 'No extra flags'
                    : flags.map(function(f) { return f.flag + ' — ' + f.reason; }).join('\n');
                var envNote = s.environmental > 0 ? '\n' + s.environmental + ' environmental (excluded)' : '';
                var title = b.profile_name + '\n' + statusCount(s, 'succeeded') + '/' + comparableTotal(s) + ' succeeded' + envNote + '\n' + flagDetail;

                return '<tr class="matrix-profile-row ' + colorCls + (lowN ? ' low-n' : '') + '" ' +
                       'data-action="go-details" data-id="' + escapeAttr(b.id) + '" title="' + escapeAttr(title) + '">' +
                       '<td class="mpr-label">' + escapeHtml(b.config.flag_summary) + '</td>' +
                       '<td class="mpr-rate">' + rate.toFixed(1) + '%</td>' +
                       '<td class="mpr-n">' + (lowN ? '⚠ ' : '') + 'N=' + comparableTotal(s) + (s.environmental > 0 ? '<span class="mpr-env" title="' + s.environmental + ' environmental failures excluded">*</span>' : '') + '</td>' +
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
        var rate = Math.round(successRate(b.stats));
        return {
            value: b.id,
            label: batchLabel(b) + '  (' + rate + '%, N=' + comparableTotal(b.stats) + ')'
        };
    });
    setDropdownOptions('details-batch-dd', opts);
}

function populateStatusFilter() {
    var dd = el('status-filter-dd');
    if (!dd) return;
    var menu = dd.querySelector('.dropdown-menu');
    if (!menu) return;
    var statuses = allStatuses();
    var labels = { succeeded: 'Succeeded', failed: 'Failed', oom_killed: 'OOM-killed',
                   timeout: 'Timeout', dep_wait: 'Dep-wait' };
    var html = '<li data-value="">All</li>';
    statuses.forEach(function(st) {
        html += '<li data-value="' + escapeAttr(st) + '">' + escapeHtml(labels[st] || st) + '</li>';
    });
    menu.innerHTML = html;
}

function loadDetailsForBatch(batchId, pushHistory, preserveFilter) {
    currentBatch = batches.find(function(b) { return b.id === batchId; });
    if (!currentBatch) return;
    currentBatchData = loadBatchData(batchId);
    if (!preserveFilter) categoryFilter = null;  // reset category filter when switching batches

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
    var compStr = '';
    if (currentBatchData) {
        var counts = {};
        var known = 0;
        currentBatchData.builds.forEach(function(bld) {
            if (!bld.component) return;
            counts[bld.component] = (counts[bld.component] || 0) + 1;
            known++;
        });
        var unknown = currentBatchData.builds.length - known;
        if (known > 0) {
            compStr = Object.keys(counts).sort().map(function(c) {
                return c + ':' + counts[c];
            }).join(' ');
            if (unknown > 0) compStr += ' (+' + unknown + ' unknown)';
            compStr = ' · ' + compStr;
        }
    }
    ctx.textContent = b.compiler_type + ' ' + b.compiler_version +
        ' · ' + b.series + ' · ' + b.config.flag_summary +
        ' (' + flagStr + ') · N=' + b.stats.total + compStr;
}

function renderDetailsStatusBar() {
    var b = currentBatch;
    if (!b) return;
    var s = b.stats;
    var rate = comparableTotal(s) > 0 ? successRate(s).toFixed(0) : 0;
    var started = new Date(b.started_at).toLocaleString();
    var totalSecs = 0;
    if (currentBatchData) {
        currentBatchData.builds.forEach(function(bld) { totalSecs += bld.duration_seconds || 0; });
    }
    var sb = el('details-status-bar');
    if (!sb) return;

    var segments = [];
    STATUS_ORDER.forEach(function(st) {
        var n = s.by_status[st];
        if (!n) return;
        var label = st === 'dep_wait' ? 'dep-wait' : st.replace('_', ' ');
        segments.push('<span class="s-st s-' + st + '">' + n + ' ' + escapeHtml(label) + '</span>');
    });
    // Any status the fixed order doesn't know about still shows.
    Object.keys(s.by_status).forEach(function(st) {
        if (STATUS_ORDER.indexOf(st) === -1 && s.by_status[st]) {
            segments.push('<span class="s-st">' + s.by_status[st] + ' ' + escapeHtml(st) + '</span>');
        }
    });
    if (s.environmental > 0) {
        segments.push('<span class="s-env" title="Environmental/infrastructure failures, excluded from the success rate">' +
            s.environmental + ' environmental</span>');
    }
    if (s.retried > 0) {
        segments.push('<span class="s-retry" title="Packages that were OOM-killed and retried at jobs=1">' +
            s.retried + ' retried</span>');
    }

    sb.innerHTML = segments.join('') +
        '<span>' + s.total + ' total</span>' +
        '<span><span class="rate-bar"><span class="rate-fill" style="width:' + rate + '%"></span></span> ' + rate + '%' + (s.environmental > 0 ? ' <span class="muted">(excl. environmental)</span>' : '') + '</span>' +
        '<span>' + fmtDuration(totalSecs) + ' total build time</span>' +
        '<span class="batch-meta">' + escapeHtml(started) + '</span>';
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

    // A clickable category bar that filters the builds table. `extraCls` adds a
    // severity/class colour; `filterValue` is what renderBuildsTable matches on.
    function bar(label, count, filterValue, extraCls, titleText) {
        var active = categoryFilter === filterValue;
        var pkgWord = count === 1 ? 'package' : 'packages';
        var defaultTitle = count + ' ' + pkgWord + ' affected — click to filter by ' + label;
        return '<div class="findings-bar-item findings-bar-clickable' + (extraCls ? ' ' + extraCls : '') +
            (active ? ' findings-bar-active' : '') + '" ' +
            'data-action="filter-category" data-cat="' + escapeAttr(filterValue) + '" ' +
            'title="' + escapeAttr(titleText || defaultTitle) + '">' +
            '<span class="findings-bar-count">' + count + '</span>' +
            '<span class="findings-bar-label">' + escapeHtml(label) + '</span>' +
            (active ? '<span class="findings-bar-x" title="Clear filter">×</span>' : '') +
            '</div>';
    }

    var html = '';

    // Error findings (from failed builds) — split toolchain vs environmental.
    if (errors.length > 0 || unanalyzed > 0) {
        var toolchainErrors = errors.filter(function(f) { return f.finding_class !== 'environmental'; });
        var envErrors = errors.filter(function(f) { return f.finding_class === 'environmental'; });

        html += '<p class="findings-section-label findings-label-error">Errors (toolchain)</p>';
        toolchainErrors.forEach(function(f) {
            html += bar(f.category, f.count, f.category, null);
        });
        if (unanalyzed > 0) {
            var uWord = unanalyzed === 1 ? 'package' : 'packages';
            html += bar('Unanalyzed (no patterns matched)', unanalyzed, '__unanalyzed__',
                'findings-bar-unanalyzed', unanalyzed + ' ' + uWord + ' failed with no matched pattern — click to filter');
        }

        if (envErrors.length > 0) {
            html += '<p class="findings-section-label findings-label-environmental" style="margin-top:6px" ' +
                'title="Infrastructure/environmental artifacts, excluded from the success rate">Environmental (excluded from rate)</p>';
            envErrors.forEach(function(f) {
                html += bar(f.category, f.count, f.category, 'findings-bar-environmental');
            });
        }
    }

    // Observation findings (from succeeded builds)
    if (observations.length > 0) {
        html += '<p class="findings-section-label findings-label-observation" style="margin-top:6px">Observations</p>';
        observations.forEach(function(f) {
            html += bar(f.category, f.count, f.category, 'findings-bar-observation');
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
    // Sibling profiles: same compiler, series, and arch.
    var siblings = batches.filter(function(s) {
        return s.compiler_type    === b.compiler_type &&
               s.compiler_version === b.compiler_version &&
               s.series           === b.series &&
               s.arch             === b.arch;
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
        var rate = comparableTotal(s) > 0 ? successRate(s).toFixed(1) : '—';
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
            '<td class="num mono">' + comparableTotal(s) + (s.environmental > 0 ? '<span class="mpr-env" title="' + s.environmental + ' environmental failures excluded">*</span>' : '') + '</td>' +
            '<td class="num mono s-pass">' + statusCount(s, 'succeeded') + '</td>' +
            '<td class="num mono s-fail">' + statusCount(s, 'failed') + (statusCount(s, 'timeout') ? '+' + statusCount(s, 'timeout') + 't' : '') + '</td>' +
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

    // Same series, arch, flag config, and compiler type; grouped by version.
    var related = batches.filter(function(s) {
        return s.series === b.series &&
               s.arch === b.arch &&
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

    var versions = Object.keys(verMap).sort(compareVersions);

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
        var rate = successRate(s);
        var lowN = comparableTotal(s) < 50;
        var isCurrent = bv.id === b.id;
        html += '<tr class="ver-row' + (isCurrent ? ' details-current-row' : '') + '"' +
            ' data-action="go-details" data-id="' + escapeAttr(bv.id) + '"' +
            ' title="Open ' + escapeAttr(bv.profile_name) + ' in Details">' +
            '<td class="mono">' + escapeHtml(v) + (isCurrent ? ' <span class="muted">(current)</span>' : '') + '</td>' +
            '<td class="num mono">' + (lowN ? '⚠ ' : '') + comparableTotal(s) + '</td>' +
            '<td class="num mono s-pass">' + statusCount(s, 'succeeded') + '</td>' +
            '<td class="num mono s-fail">' + statusCount(s, 'failed') + '</td>' +
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
        if (filt && b.package.toLowerCase().indexOf(filt) === -1 &&
            (!b.component || b.component.toLowerCase().indexOf(filt) === -1)) return false;
        if (statFilt && b.status !== statFilt) return false;
        if (categoryFilter) {
            if (categoryFilter === '__unanalyzed__') {
                if (b.status === 'succeeded' || b.finding_count > 0) return false;
            } else if (!b.categories || b.categories.indexOf(categoryFilter) === -1) {
                return false;
            }
        }
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

    // Result count / active filter line.
    var rcEl = el('builds-result-count');
    if (rcEl) {
        var parts = [builds.length + ' of ' + currentBatchData.builds.length + ' packages'];
        if (categoryFilter) {
            var label = categoryFilter === '__unanalyzed__' ? 'Unanalyzed' : categoryFilter;
            parts.push('<button class="builds-filter-chip" data-action="filter-clear">' +
                '<span class="builds-filter-chip-label">' + escapeHtml(label) + '</span>' +
                '<span class="findings-bar-x">×</span></button>');
        }
        rcEl.innerHTML = parts.join(' · ');
    }

    var html = '';
    builds.forEach(function(b) {
        var issues = b.finding_count > 0 ? String(b.finding_count)
            : b.status === 'succeeded' ? '<span class="cell-hint" data-hint="No issues detected">0</span>'
            : (b.status === 'failed' || b.status === 'timeout' || b.status === 'dep_wait')
              ? '<span class="cell-hint" data-hint="Build did not complete">n/a</span>' : '-';

        // Row highlight: environmental-only failures get a distinct class.
        var rowCls = '';
        if (b.status !== 'succeeded') {
            rowCls = b.env_only ? 'build-row-env' : 'build-row-fail';
        }

        // Status cell: environmental-only failures display as "environmental".
        var isEnv = b.status !== 'succeeded' && b.env_only;
        var stLabel = isEnv ? 'environmental' : (b.status === 'oom_killed' ? 'oom-killed' : b.status.replace('_', ' '));
        var stCls = isEnv ? 'environmental' : b.status;

        var retryBadge = '';
        if (b.retries.length > 0) {
            var hist = b.retries.map(function(a) {
                var j = a.jobs != null ? ' @ ' + a.jobs + ' jobs' : '';
                return a.status + j;
            }).join(' → ');
            retryBadge = ' <span class="retry-badge" title="' +
                escapeAttr(b.retries.length + ' attempts: ' + hist) + '">⟳</span>';
        }

        var compTag = b.component
            ? ' <span class="comp-tag" title="Archive component">' + escapeHtml(b.component) + '</span>'
            : '';

        html += '<tr class="' + rowCls + '">' +
            '<td><span class="pkg-name">' + escapeHtml(b.package) + '</span>' + retryBadge + compTag + '</td>' +
            '<td><span class="st st-' + stCls + '">' + stLabel + '</span></td>' +
            '<td class="num mono">' + (b.duration_seconds ? fmtDuration(b.duration_seconds) : '-') + '</td>' +
            '<td class="num mono">' + (b.peak_memory_mb ? b.peak_memory_mb + ' MB' : '-') + '</td>' +
            '<td class="num">' + issues + '</td>' +
            '<td>' +
                (b.finding_count > 0 ? '<button class="btn-link" data-action="issues" data-id="' + b.id + '">issues</button> ' : '') +
                '<button class="btn-link" data-action="log" data-id="' + b.id + '" data-pkg="' + escapeAttr(b.package) + '">log</button>' +
            '</td></tr>';
    });
    if (html === '') {
        html = '<tr><td colspan="6" class="muted" style="padding:1rem;text-align:center">No packages match the current filters.</td></tr>';
    }
    tbody.innerHTML = html;
}

// Set (or toggle off) the Issue-Category filter and re-render.
// Activating a filter pushes a history entry so the back button can clear it;
// clearing a filter replaces the current entry (no extra back step needed).
function setCategoryFilter(cat) {
    var next = (categoryFilter === cat) ? null : cat;
    var batchId = currentBatch ? currentBatch.id : null;
    if (next !== null) {
        history.pushState({ tab: 'details', batchId: batchId, categoryFilter: next }, '');
    } else {
        history.replaceState({ tab: 'details', batchId: batchId, categoryFilter: null }, '');
    }
    categoryFilter = next;
    renderDetailsFindings();
    renderBuildsTable();
}

function clearCategoryFilter() {
    if (categoryFilter === null) return;
    var batchId = currentBatch ? currentBatch.id : null;
    history.replaceState({ tab: 'details', batchId: batchId, categoryFilter: null }, '');
    categoryFilter = null;
    renderDetailsFindings();
    renderBuildsTable();
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
        var label = batchLabel(b);
        if (filter && label.toLowerCase().indexOf(filter) === -1) return;
        var rate = Math.round(successRate(b.stats));
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
        var finalWhere = exportHas.attemptNumber ? " AND " + FINAL_ATTEMPT_WHERE : "";
        var builds = dbQuery(
            "SELECT b.source_package, b.status, b.build_duration_seconds AS dur, b.peak_memory_mb AS mem, b.id " +
            "FROM builds b WHERE b.batch_id = ?" + finalWhere,
            [b.id]
        );
        // Top category per failing build, plus whether the build's findings are
        // all environmental (infra artifact, not a toolchain failure).
        var cats = {};
        var envOnly = {};
        dbQuery(
            "SELECT b.source_package, bf.category, bf.finding_class, COUNT(*) as cnt " +
            "FROM build_findings bf JOIN builds b ON bf.build_id = b.id " +
            "WHERE b.batch_id = ? GROUP BY b.source_package, bf.category",
            [b.id]
        ).forEach(function(r) {
            if (!cats[r.source_package] || r.cnt > cats[r.source_package].cnt)
                cats[r.source_package] = { category: r.category, cnt: Number(r.cnt) };
            // Track environmental-only: starts true on first finding, flipped
            // false if any non-environmental finding appears.
            if (envOnly[r.source_package] === undefined) envOnly[r.source_package] = true;
            if (r.finding_class !== 'environmental') envOnly[r.source_package] = false;
        });
        var map = {};
        builds.forEach(function(bld) { map[bld.source_package] = bld; });
        return { batch: b, map: map, cats: cats, envOnly: envOnly };
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
        var label = escapeHtml(b.profile_name) + '<br><span class="compare-col-series">' + escapeHtml(b.series + (multiArch && b.arch ? ' · ' + b.arch : '') + ' · ' + b.config.flag_summary) + '</span>';
        headerHtml += '<th class="compare-col-batch" style="min-width:' + colW + 'px" title="' + escapeAttr(batchLabel(b)) + '">' + label + '</th>';
    });
    headerHtml += '<th class="actions-col">Log</th>';

    function pkgRow(pkg) {
        var cells = batchData.map(function(d) {
            var bld = d.map[pkg];
            if (!bld) return '<td class="compare-cell compare-cell-missing"><span class="muted">—</span></td>';
            var st = bld.status;
            var isEnv = st !== 'succeeded' && d.envOnly[pkg];
            var cat = (st !== 'succeeded' && d.cats[pkg]) ? d.cats[pkg].category : null;
            var cls = st === 'succeeded' ? 'compare-cell-ok'
                : isEnv ? 'compare-cell-env'
                : st === 'failed' ? 'compare-cell-fail' : 'compare-cell-other';
            var label = isEnv ? 'environmental' : st;
            return '<td class="compare-cell ' + cls + '">' +
                '<span class="st st-' + (isEnv ? 'environmental' : st) + '">' + label + '</span>' +
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
        return escapeHtml(batchLabel(b));
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
    if (action === 'filter-category') setCategoryFilter(btn.getAttribute('data-cat'));
    if (action === 'filter-clear') clearCategoryFilter();
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
        "SELECT category, description, excerpt, line_number, severity, finding_class " +
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
                var envBadge = f.finding_class === 'environmental'
                    ? ' <span class="finding-class-badge">environmental · excluded from rate</span>' : '';
                var cls = f.finding_class === 'environmental' ? 'finding-detail-environmental' : 'finding-detail-error';
                return '<div class="finding-detail ' + cls + '">' +
                    '<h4>' + escapeHtml(f.category) + envBadge + '</h4>' +
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
    var term = searchTerm ? searchTerm.toLowerCase() : null;
    var hitCount = 0;
    var html = '';
    for (var i = 0; i < lines.length; i++) {
        var num = String(i + 1).padStart(numWidth);
        // Highlight on the raw line, escape each chunk separately: matching
        // against escaped text would break on terms containing & < >.
        var content;
        if (term) {
            var lower = lines[i].toLowerCase();
            var out = '', pos = 0, idx;
            while ((idx = lower.indexOf(term, pos)) !== -1) {
                hitCount++;
                out += escapeHtml(lines[i].substring(pos, idx)) +
                    '<span class="search-hit">' + escapeHtml(lines[i].substring(idx, idx + term.length)) + '</span>';
                pos = idx + term.length;
            }
            content = out + escapeHtml(lines[i].substring(pos));
        } else {
            content = escapeHtml(lines[i]);
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

var logSearchTimer = null;
function handleLogSearch() {
    if (logSearchTimer) clearTimeout(logSearchTimer);
    logSearchTimer = setTimeout(function() {
        logSearchTimer = null;
        var ls = el('log-search');
        renderLog(currentLogText, ls && ls.value.trim() || null);
    }, 150);
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
        // Restore the category filter from history state before loading the batch,
        // so loadDetailsForBatch does not clobber it.
        categoryFilter = e.state.categoryFilter || null;
        loadDetailsForBatch(e.state.batchId, false, true);
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

// Natural version order: numeric runs compare numerically ("9" < "10",
// "2.9" < "2.28"), everything else lexically.
function compareVersions(a, b) {
    var ai = String(a).split(/(\d+)/), bi = String(b).split(/(\d+)/);
    for (var i = 1; i < Math.max(ai.length, bi.length); i += 2) {
        var x = ai[i], y = bi[i];
        if (x === undefined || y === undefined) return ai.length - bi.length;
        var d = parseInt(x, 10) - parseInt(y, 10);
        if (d) return d;
        var p = ai[i + 1], q = bi[i + 1];
        if (p !== q) return (p || '') < (q || '') ? -1 : 1;
    }
    return 0;
}

// Label for a batch in selectors and table headers: profile, series, arch
// (arch only when the data spans multiple).
function batchLabel(b) {
    return b.profile_name + ' · ' + b.series + (multiArch && b.arch ? ' · ' + b.arch : '');
}

function cssAttrEscape(s) {
    return String(s).replace(/\\/g, '\\\\').replace(/"/g, '\\"');
}

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

// Denominator for a fair compiler comparison: total builds minus environmental-
// only failures (infra artifacts that aren't a toolchain result).
function comparableTotal(s) {
    return Math.max(0, (s.total || 0) - (s.environmental || 0));
}

function successRate(s) {
    var denom = comparableTotal(s);
    return denom > 0 ? (statusCount(s, 'succeeded') / denom) * 100 : 0;
}

function statusCount(s, status) {
    return (s.by_status && s.by_status[status]) || 0;
}

// Statuses present across all batches, for the filter dropdown.
function allStatuses() {
    var seen = {};
    batches.forEach(function(b) {
        Object.keys(b.stats.by_status || {}).forEach(function(st) { seen[st] = true; });
    });
    var known = STATUS_ORDER.filter(function(st) { return seen[st]; });
    var extra = Object.keys(seen).filter(function(st) { return STATUS_ORDER.indexOf(st) === -1; }).sort();
    return known.concat(extra);
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
    var li = menu ? menu.querySelector('li[data-value="' + cssAttrEscape(value) + '"]') : null;
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
