/** Preserve click/key ordering while keeping only the latest unsent movement.
 * Scroll deltas accumulate in one pending event instead of a stale-event queue. */
export function createBrowserInputQueue(invoke) {
  var pending = [];
  var sending = false;

  function drain() {
    if (sending || !pending.length) return;
    sending = true;
    var entry = pending.shift();
    Promise.resolve().then(function () { return invoke(entry.command, entry.args); }).then(
      function (value) { entry.waiters.forEach(function (w) { w.resolve(value); }); },
      function (error) { entry.waiters.forEach(function (w) { w.reject(error); }); }
    ).finally(function () { sending = false; drain(); });
  }

  function enqueue(command, args) {
    return new Promise(function (resolve, reject) {
      var last = pending[pending.length - 1];
      var event = command === 'browser_pointer' && args.event;
      var previous = last && last.command === command && last.args.event;
      // Never merge across button/key boundaries or changes in modifiers.
      if (event && previous && event.type === previous.type &&
          event.modifiers === previous.modifiers && event.buttons === previous.buttons &&
          (event.type === 'mouseMoved' || event.type === 'mouseWheel')) {
        last.args = { event: Object.assign({}, event) };
        if (event.type === 'mouseWheel') {
          last.args.event.deltaX += previous.deltaX;
          last.args.event.deltaY += previous.deltaY;
        }
        last.waiters.push({ resolve: resolve, reject: reject });
      } else {
        pending.push({ command: command, args: args, waiters: [{ resolve: resolve, reject: reject }] });
      }
      drain();
    });
  }
  // Drop unsent events when their page is no longer active.
  enqueue.clear = function () {
    pending.splice(0).forEach(function (entry) {
      entry.waiters.forEach(function (w) { w.resolve(undefined); });
    });
  };
  return enqueue;
}
