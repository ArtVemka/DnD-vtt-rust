'use strict';

/**
 * Небольшой 3D-рендерер костей на canvas.
 *
 * Без внешних библиотек: у каждой кости свой набор вершин и граней, painter's
 * algorithm и простое освещение. Этого хватает, чтобы результат читался как
 * объёмная кость, а не как плоский шестиугольник.
 */
(function initDice3D(global) {
  const GOLD = [236, 202, 134];
  const DARK = [96, 58, 22];
  const INK = 'rgba(74, 45, 16, 0.92)';
  const LIGHT = normalize([-0.45, 0.72, 0.62]);
  const CACHE = new Map();

  function normalize(vector) {
    const length = Math.hypot(vector[0], vector[1], vector[2]) || 1;
    return [vector[0] / length, vector[1] / length, vector[2] / length];
  }

  function subtract(a, b) {
    return [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
  }

  function cross(a, b) {
    return [
      a[1] * b[2] - a[2] * b[1],
      a[2] * b[0] - a[0] * b[2],
      a[0] * b[1] - a[1] * b[0]
    ];
  }

  function dot(a, b) {
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
  }

  function tetrahedron() {
    return {
      vertices: [[1, 1, 1], [1, -1, -1], [-1, 1, -1], [-1, -1, 1]],
      faces: [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]]
    };
  }

  function cube() {
    return {
      vertices: [
        [-1, -1, -1], [1, -1, -1], [1, 1, -1], [-1, 1, -1],
        [-1, -1, 1], [1, -1, 1], [1, 1, 1], [-1, 1, 1]
      ],
      faces: [
        [0, 1, 2, 3], [5, 4, 7, 6], [4, 0, 3, 7],
        [1, 5, 6, 2], [3, 2, 6, 7], [4, 5, 1, 0]
      ]
    };
  }

  function octahedron() {
    return {
      vertices: [
        [1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1]
      ],
      faces: [
        [0, 2, 4], [0, 4, 3], [0, 3, 5], [0, 5, 2],
        [1, 4, 2], [1, 3, 4], [1, 5, 3], [1, 2, 5]
      ]
    };
  }

  function icosahedron() {
    const phi = (1 + Math.sqrt(5)) / 2;
    return {
      vertices: [
        [-1, phi, 0], [1, phi, 0], [-1, -phi, 0], [1, -phi, 0],
        [0, -1, phi], [0, 1, phi], [0, -1, -phi], [0, 1, -phi],
        [phi, 0, -1], [phi, 0, 1], [-phi, 0, -1], [-phi, 0, 1]
      ],
      faces: [
        [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11],
        [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
        [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9],
        [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1]
      ]
    };
  }

  /** d10 и d12: бипирамида с нужным числом граней. */
  function bipyramid(count) {
    const vertices = [[0, 1.05, 0], [0, -1.05, 0]];
    for (let index = 0; index < count; index += 1) {
      const angle = (index / count) * Math.PI * 2;
      vertices.push([Math.cos(angle), 0, Math.sin(angle)]);
    }
    const faces = [];
    for (let index = 0; index < count; index += 1) {
      const next = (index + 1) % count;
      faces.push([0, 2 + index, 2 + next]);
      faces.push([1, 2 + next, 2 + index]);
    }
    return { vertices, faces };
  }

  function geometry(sides) {
    if (CACHE.has(sides)) return CACHE.get(sides);
    let shape;
    if (sides === 4) shape = tetrahedron();
    else if (sides === 8) shape = octahedron();
    else if (sides === 10) shape = bipyramid(5);
    else if (sides === 12) shape = bipyramid(6);
    else if (sides === 20) shape = icosahedron();
    else shape = cube();
    const radius = Math.max(...shape.vertices.map((vertex) => Math.hypot(...vertex))) || 1;
    shape.vertices = shape.vertices.map((vertex) => vertex.map((value) => value / radius));
    CACHE.set(sides, shape);
    return shape;
  }

  function rotate(vertex, ax, ay, az) {
    let [x, y, z] = vertex;
    let cos = Math.cos(ay);
    let sin = Math.sin(ay);
    [x, z] = [x * cos + z * sin, -x * sin + z * cos];
    cos = Math.cos(ax);
    sin = Math.sin(ax);
    [y, z] = [y * cos - z * sin, y * sin + z * cos];
    cos = Math.cos(az);
    sin = Math.sin(az);
    [x, y] = [x * cos - y * sin, x * sin + y * cos];
    return [x, y, z];
  }

  function project(vertex, size) {
    const camera = 3.4;
    const factor = camera / (camera - vertex[2]);
    const scale = size * 0.42;
    return [size / 2 + vertex[0] * scale * factor, size / 2 - vertex[1] * scale * factor];
  }

  function shade(intensity) {
    const mix = Math.max(0, Math.min(1, intensity));
    return `rgb(${Math.round(DARK[0] + (GOLD[0] - DARK[0]) * mix)}, `
      + `${Math.round(DARK[1] + (GOLD[1] - DARK[1]) * mix)}, `
      + `${Math.round(DARK[2] + (GOLD[2] - DARK[2]) * mix)})`;
  }

  function draw(context, shape, size, ax, ay, az) {
    context.clearRect(0, 0, size, size);

    const glow = context.createRadialGradient(
      size / 2, size / 2, size * 0.12,
      size / 2, size / 2, size * 0.54
    );
    glow.addColorStop(0, 'rgba(231, 203, 146, 0.22)');
    glow.addColorStop(1, 'rgba(231, 203, 146, 0)');
    context.fillStyle = glow;
    context.fillRect(0, 0, size, size);

    // Тень под костью.
    context.save();
    context.translate(size / 2, size * 0.88);
    context.scale(1, 0.28);
    const shadow = context.createRadialGradient(0, 0, 0, 0, 0, size * 0.36);
    shadow.addColorStop(0, 'rgba(0, 0, 0, 0.42)');
    shadow.addColorStop(1, 'rgba(0, 0, 0, 0)');
    context.fillStyle = shadow;
    context.beginPath();
    context.arc(0, 0, size * 0.36, 0, Math.PI * 2);
    context.fill();
    context.restore();

    const rotated = shape.vertices.map((vertex) => rotate(vertex, ax, ay, az));
    const faces = shape.faces.map((indices) => {
      const points = indices.map((index) => rotated[index]);
      let normal = normalize(cross(subtract(points[1], points[0]), subtract(points[2], points[0])));
      const centroid = points.reduce(
        (sum, point) => [sum[0] + point[0], sum[1] + point[1], sum[2] + point[2]],
        [0, 0, 0]
      ).map((value) => value / points.length);
      // Наружу, а не внутрь: иначе передние грани получают тень, и кость
      // выглядит тёмным силуэтом с золотым ободком.
      if (dot(normal, centroid) < 0) normal = normal.map((value) => -value);
      const depth = points.reduce((sum, point) => sum + point[2], 0) / points.length;
      return { points, normal, depth };
    }).sort((left, right) => left.depth - right.depth);

    const line = Math.max(1, size * 0.014);
    for (const face of faces) {
      const projected = face.points.map((point) => project(point, size));
      context.beginPath();
      projected.forEach(([x, y], index) => {
        if (index === 0) context.moveTo(x, y);
        else context.lineTo(x, y);
      });
      context.closePath();
      const light = 0.32 + 0.68 * Math.max(0, dot(face.normal, LIGHT));
      context.fillStyle = shade(light);
      context.fill();
      context.strokeStyle = INK;
      context.lineWidth = line;
      context.stroke();

      // Тонкий блик на гранях, повёрнутых к свету.
      if (face.normal[2] > 0.25) {
        context.strokeStyle = 'rgba(255, 244, 210, 0.34)';
        context.lineWidth = Math.max(0.6, line * 0.5);
        context.stroke();
      }
    }
  }

  /**
   * @param {HTMLElement} host
   * @param {number} sides
   * @param {string} value
   * @param {{size?: number, mini?: boolean}} [options]
   */
  function mount(host, sides, value, options = {}) {
    if (!host) return null;
    const size = options.size || 96;
    const mini = Boolean(options.mini);
    const shape = geometry(Number(sides) || 20);
    const ratio = Math.min(2, global.devicePixelRatio || 1);

    host.classList.add('dice3d-host');
    host.innerHTML = '';

    const canvas = document.createElement('canvas');
    canvas.className = 'dice3d';
    canvas.width = Math.round(size * ratio);
    canvas.height = Math.round(size * ratio);
    canvas.style.width = `${size}px`;
    canvas.style.height = `${size}px`;
    host.appendChild(canvas);

    if (value) {
      const label = document.createElement('span');
      label.className = 'dice3d__value';
      label.textContent = value;
      host.appendChild(label);
    }

    const context = canvas.getContext('2d');
    if (!context) return null;
    context.setTransform(ratio, 0, 0, ratio, 0, 0);

    const started = performance.now();
    const spin = mini ? 900 : 1250;
    const idle = mini ? 900 : 3200;
    const baseX = mini ? -0.4 : -0.28;
    const baseY = mini ? 0.6 : 0.48;
    let frame = 0;

    const tick = (now) => {
      if (!host.isConnected) return;
      const elapsed = (now - started) / 1000;
      const spinLeft = Math.max(0, 1 - elapsed / (spin / 1000));
      const ax = baseX
        + Math.sin(elapsed * 1.3) * 0.1
        + spinLeft * Math.sin(elapsed * 26) * 1.7;
      const ay = baseY
        + elapsed * 0.42
        + spinLeft * Math.cos(elapsed * 21) * 2.2;
      const az = Math.sin(elapsed * 0.8) * 0.08 + spinLeft * Math.sin(elapsed * 17) * 0.4;
      draw(context, shape, size, ax, ay, az);
      if (now - started < idle) {
        frame = requestAnimationFrame(tick);
      }
    };
    frame = requestAnimationFrame(tick);

    return () => cancelAnimationFrame(frame);
  }

  global.DnDDice3D = { mount };
}(window));
