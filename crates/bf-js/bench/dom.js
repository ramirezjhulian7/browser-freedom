// Objetos al estilo de nodos del DOM: muchas propiedades, árbol, recorridos y escrituras.
function Node(type, name, parent) {
  this.nodeType = type;
  this.nodeName = name;
  this.parentNode = parent;
  this.firstChild = null;
  this.lastChild = null;
  this.previousSibling = null;
  this.nextSibling = null;
  this.childCount = 0;
  this.id = '';
  this.className = '';
  this.textContent = '';
  this.hidden = false;
  this.tabIndex = -1;
  this.offsetTop = 0;
  this.offsetLeft = 0;
  this.offsetWidth = 0;
  this.offsetHeight = 0;
  this.dataset = null;
}
function append(parent, child) {
  if (parent.lastChild) {
    parent.lastChild.nextSibling = child;
    child.previousSibling = parent.lastChild;
  } else {
    parent.firstChild = child;
  }
  parent.lastChild = child;
  parent.childCount++;
}
function build(parent, depth) {
  for (var i = 0; i < 6; i++) {
    var n = new Node(1, i % 2 ? 'DIV' : 'SPAN', parent);
    n.id = 'n' + depth + '_' + i;
    n.className = i % 3 === 0 ? 'a b' : 'c';
    append(parent, n);
    if (depth < 5) build(n, depth + 1);
    else append(n, new Node(3, '#text', n));
  }
}
function layout(n, top) {
  var h = 0;
  for (var c = n.firstChild; c; c = c.nextSibling) {
    c.offsetTop = top + h;
    c.offsetLeft = n.offsetLeft + 4;
    c.offsetWidth = n.offsetWidth ? n.offsetWidth - 8 : 800;
    c.offsetHeight = c.nodeType === 3 ? 16 : layout(c, top + h);
    h += c.offsetHeight;
  }
  return h;
}
function count(n) {
  var k = n.nodeType === 1 && n.className.length > 1 ? 1 : 0;
  for (var c = n.firstChild; c; c = c.nextSibling) k += count(c);
  return k;
}
function run() {
  var root = new Node(9, '#document', null);
  build(root, 0);
  var total = 0;
  for (var r = 0; r < 3; r++) total += layout(root, 0) + count(root);
  return total;
}
check(run(), 2295474);
