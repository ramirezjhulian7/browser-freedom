// E1: HTMLSelectElement `add(element, before)` y `remove(index)` (HTML §4.10.7, con los pasos de
// HTMLOptionsCollection `add` y `remove`, §2.6.3). Sobre las operaciones del DOM: `insertBefore`
// hace el "pre-insert" (y sus errores) y avisa a los observadores.
(function (global) {
  'use strict';
  const Select = global.HTMLSelectElement;
  if (!Select) return;
  const P = Select.prototype;
  const childRemove = global.Element.prototype.remove;
  const DOMException = global.DOMException;
  const check = (o) => {
    if (!(o instanceof Select)) throw new TypeError('Illegal invocation');
  };
  const options = (s) => Array.from(s.querySelectorAll('option')).filter((o) => {
    // §4.10.7 "list of options": hijos `option` y los de un `optgroup` hijo.
    const p = o.parentNode;
    return p === s || (p && p.localName === 'optgroup' && p.parentNode === s);
  });
  const methods = {
    add(element, before = null) {
      check(this);
      if (arguments.length < 1) throw new TypeError("Failed to execute 'add' on 'HTMLSelectElement': 1 argument required.");
      const isOpt = element instanceof global.HTMLOptionElement || element instanceof global.HTMLOptGroupElement;
      if (!isOpt) throw new TypeError("Failed to execute 'add' on 'HTMLSelectElement': The provided value is not of type '(HTMLOptionElement or HTMLOptGroupElement)'.");
      // 1. Si `element` es un antecesor del select: HierarchyRequestError.
      if (element.contains(this)) throw new DOMException('The new child element contains the parent.', 'HierarchyRequestError');
      if (typeof before === 'number' || (before !== null && !(before instanceof global.HTMLElement))) {
        before = options(this)[Number(before)] || null;
      }
      // 2. `before` tiene que ser descendiente del select.
      if (before !== null && !this.contains(before)) throw new DOMException('The node before which the new node is to be inserted is not a child of this node.', 'NotFoundError');
      // 3. Mismo elemento: nada.
      if (element === before) return;
      const parent = before !== null ? before.parentNode : this;
      parent.insertBefore(element, before);
    },
    remove(index) {
      check(this);
      // Sin argumentos es `ChildNode.remove()` (DOM §5.2.2).
      if (arguments.length === 0) return childRemove.call(this);
      const i = Number(index);
      const list = options(this);
      if (i >= 0 && i < list.length) list[i].remove();
    },
  };
  for (const [name, f] of Object.entries(methods)) {
    Object.defineProperty(P, name, { value: f, writable: true, enumerable: true, configurable: true });
  }
})
