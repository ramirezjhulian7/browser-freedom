// Costo de una llamada (E8): una función de dos argumentos sin trabajo, 3 M veces.
function add(a, b) {
  return a + b;
}
function run(n) {
  var s = 0;
  for (var i = 0; i < n; i++) {
    s = add(s, i);
  }
  return s;
}
check(run(3000000), 4499998500000);
