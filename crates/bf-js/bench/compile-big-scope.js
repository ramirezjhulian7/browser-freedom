// E1: compilar un scope con miles de variables y miles de referencias a ellas, como el
// código minificado de los bundles grandes (YouTube pasaba 25 s compilando: el compilador
// resolvía cada nombre recorriendo los bindings del scope). Se compila con `new Function`.
var N = 6000;
var parts = [];
for (var i = 0; i < N; i++) parts.push('var v' + i + ' = ' + i + ';');
var sum = [];
for (var j = 0; j < N; j++) sum.push('v' + j);
parts.push('return ' + sum.join(' + ') + ';');
var f = new Function(parts.join('\n'));
check(f(), (N * (N - 1)) / 2);
