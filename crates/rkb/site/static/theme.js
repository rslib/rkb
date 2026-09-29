// Sets the saved theme before the first paint; loaded without defer in <head>.
try { document.documentElement.dataset.theme = localStorage.getItem("rkb-theme") || "system"; } catch (e) { /* the default theme */ }
