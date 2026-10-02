(() => {
  if (globalThis.__comradeCosmeticInstalled) return;
  globalThis.__comradeCosmeticInstalled = true;
  let timer;
  const update = () => {
    if (timer) return;
    timer = setTimeout(() => {
      timer = null;
      const classes = new Set(), ids = new Set();
      const nodes = document.querySelectorAll('[class], [id]');
      for (let i = 0; i < Math.min(nodes.length, 5000); i++) {
        for (const name of nodes[i].classList) if (classes.size < 2000) classes.add(name);
        if (nodes[i].id && ids.size < 2000) ids.add(nodes[i].id);
      }
      globalThis.__comradeAdblock(JSON.stringify({ url: location.href, classes: [...classes], ids: [...ids] }));
    }, 150);
  };
  new MutationObserver(records => {
    if (records.some(record => record.target.id !== '__comradeAdblockStyle' &&
      ![...record.addedNodes, ...record.removedNodes].some(node => node.id === '__comradeAdblockStyle'))) update();
  }).observe(document, { childList: true, subtree: true, attributes: true, attributeFilter: ['class', 'id'] });
  globalThis.addEventListener('__comradeAdblockRefresh', update);
  update();
})();
