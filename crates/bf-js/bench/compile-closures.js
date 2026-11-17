// E1: muchas funciones anidadas que leen nombres de un scope grande (el patrón de los
// módulos de un bundle que usan las variables del IIFE que los envuelve): cada referencia se
// resuelve subiendo por los scopes.
var N = 3000;
var src = [];
for (var i = 0; i < N; i++) src.push('var a' + i + ' = ' + (i % 7) + ';');
src.push('var fs = [];');
for (var k = 0; k < N; k++) {
  src.push('fs.push(function () { return a' + k + ' + a' + ((k * 31) % N) + ' + a' + ((k * 17) % N) + '; });');
}
src.push('var t = 0; for (var q = 0; q < fs.length; q++) t += fs[q](); return t;');
var g = new Function(src.join('\n'));
var expect = 0;
for (var k2 = 0; k2 < N; k2++) expect += (k2 % 7) + (((k2 * 31) % N) % 7) + (((k2 * 17) % N) % 7);
check(g(), expect);
