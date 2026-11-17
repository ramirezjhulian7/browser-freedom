// E1: un bundle al estilo webpack: un IIFE con un objeto de módulos, cada uno una función con
// sus propias variables locales y referencias a helpers del IIFE. Mide compilar y ejecutar.
var M = 1500;
var src = ['var h0 = function (x) { return x + 1; }, h1 = function (x) { return x * 2; }, h2 = function (x) { return x - 3; };', 'var mods = {};'];
for (var i = 0; i < M; i++) {
  src.push('mods[' + i + '] = function (e) { var a = ' + i + ', b = h0(a), c = h1(b), d = h2(c); ' +
    'for (var j = 0; j < 3; j++) { a = h0(a); } e.v = a + b + c + d; };');
}
src.push('var t = 0; for (var k in mods) { var e = {}; mods[k](e); t += e.v; } return t;');
var run = new Function(src.join('\n'));
var expect = 0;
for (var n = 0; n < M; n++) { var a = n, b = a + 1, c = b * 2, d = c - 3; a = a + 3; expect += a + b + c + d; }
check(run(), expect);
