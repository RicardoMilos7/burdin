// The search box of `burdin doc` (spec/doc.md, DC18). The index is `search-index.js`, a script that sets
// `window.BURDIN_DOC_INDEX`, so the site works when opened from disk. Everything runs in the browser.
(function () {
    "use strict";
    var index = window.BURDIN_DOC_INDEX || [];
    var root = window.BURDIN_DOC_ROOT || "";
    var input = document.getElementById("search");
    var results = document.getElementById("search-results");
    var content = document.getElementById("content");
    if (!input || !results || !content) {
        return;
    }

    // Lower is better: an exact name, a name that starts with the query, a name that contains it, a path that
    // contains it. Anything else does not match.
    function score(entry, query) {
        var name = entry.n.toLowerCase();
        var path = entry.p.toLowerCase();
        if (name === query) {
            return 0;
        }
        if (name.indexOf(query) === 0) {
            return 1;
        }
        if (name.indexOf(query) !== -1) {
            return 2;
        }
        if (path.indexOf(query) !== -1) {
            return 3;
        }
        return -1;
    }

    function search(query) {
        var found = [];
        for (var i = 0; i < index.length; i++) {
            var s = score(index[i], query);
            if (s >= 0) {
                found.push({ entry: index[i], score: s });
            }
        }
        found.sort(function (a, b) {
            if (a.score !== b.score) {
                return a.score - b.score;
            }
            return a.entry.p < b.entry.p ? -1 : a.entry.p > b.entry.p ? 1 : 0;
        });
        return found.slice(0, 50);
    }

    function show(found, query) {
        results.textContent = "";
        if (found.length === 0) {
            var none = document.createElement("p");
            none.textContent = "No results for “" + query + "”.";
            results.appendChild(none);
        }
        found.forEach(function (f) {
            var a = document.createElement("a");
            a.className = "result";
            a.href = root + f.entry.u;
            var strong = document.createElement("strong");
            strong.textContent = f.entry.p;
            var kind = document.createElement("small");
            kind.textContent = f.entry.k + (f.entry.d ? " — " + f.entry.d : "");
            a.appendChild(strong);
            a.appendChild(kind);
            results.appendChild(a);
        });
    }

    function update() {
        var query = input.value.trim().toLowerCase();
        if (query === "") {
            results.hidden = true;
            content.hidden = false;
            return;
        }
        show(search(query), input.value.trim());
        results.hidden = false;
        content.hidden = true;
    }

    input.addEventListener("input", update);
    input.addEventListener("keydown", function (event) {
        if (event.key === "Escape") {
            input.value = "";
            update();
        } else if (event.key === "Enter") {
            var first = results.querySelector("a.result");
            if (first) {
                window.location.href = first.href;
            }
        }
    });
    document.addEventListener("keydown", function (event) {
        if (event.key === "/" && document.activeElement !== input) {
            event.preventDefault();
            input.focus();
        }
    });
})();
