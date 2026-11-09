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

  function project(vertex, size, lift = 0) {
    const camera = 3.4;
    const factor = camera / (camera - vertex[2]);
    // Подброшенная кость ближе к камере: чуть крупнее и выше на экране.
    const scale = size * 0.42 * (1 + lift * 0.06);
    return [
      size / 2 + vertex[0] * scale * factor,
      size / 2 - lift * size * 0.45 - vertex[1] * scale * factor
    ];
  }

  function shade(intensity) {
    const mix = Math.max(0, Math.min(1, intensity));
    return `rgb(${Math.round(DARK[0] + (GOLD[0] - DARK[0]) * mix)}, `
      + `${Math.round(DARK[1] + (GOLD[1] - DARK[1]) * mix)}, `
      + `${Math.round(DARK[2] + (GOLD[2] - DARK[2]) * mix)})`;
  }

  function polygonArea(points) {
    let area = 0;
    for (let index = 0; index < points.length; index += 1) {
      const current = points[index];
      const next = points[(index + 1) % points.length];
      area += current[0] * next[1] - next[0] * current[1];
    }
    return Math.abs(area) / 2;
  }

  function polygonInradius(points, cx, cy) {
    let min = Infinity;
    for (let index = 0; index < points.length; index += 1) {
      const a = points[index];
      const b = points[(index + 1) % points.length];
      const length = Math.hypot(b[0] - a[0], b[1] - a[1]) || 1;
      const distance = Math.abs(
        (b[1] - a[1]) * cx - (b[0] - a[0]) * cy + b[0] * a[1] - b[1] * a[0]
      ) / length;
      min = Math.min(min, distance);
    }
    return Number.isFinite(min) ? min : 0;
  }

  function uprightAngle(points, cx, cy) {
    let best = null;
    let bestLength = 0;
    for (let index = 0; index < points.length; index += 1) {
      const a = points[index];
      const b = points[(index + 1) % points.length];
      const length = Math.hypot(b[0] - a[0], b[1] - a[1]);
      if (length > bestLength) {
        bestLength = length;
        best = [b[0] - a[0], b[1] - a[1]];
      }
    }
    if (!best) return 0;
    let angle = Math.atan2(best[1], best[0]);
    // Держим цифру читаемой: не вверх ногами.
    if (angle > Math.PI / 2) angle -= Math.PI;
    if (angle < -Math.PI / 2) angle += Math.PI;
    return angle;
  }

  function pointInside(points, x, y) {
    let inside = false;
    for (let index = 0, previous = points.length - 1; index < points.length; previous = index++) {
      const [xi, yi] = points[index];
      const [xj, yj] = points[previous];
      if ((yi > y) !== (yj > y) && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) {
        inside = !inside;
      }
    }
    return inside;
  }

  function boxFits(points, cx, cy, halfWidth, halfHeight, angle) {
    const cos = Math.cos(angle);
    const sin = Math.sin(angle);
    for (const sx of [-1, 1]) {
      for (const sy of [-1, 1]) {
        const x = cx + sx * halfWidth * cos - sy * halfHeight * sin;
        const y = cy + sx * halfWidth * sin + sy * halfHeight * cos;
        if (!pointInside(points, x, y)) return false;
      }
    }
    return true;
  }

  function bestAngle(points, cx, cy, halfWidth, halfHeight, preferred) {
    // Сначала пробуем ровную цифру: она читается лучше всего. Если не влезает
    // в узкую грань — доворачиваем по длинной стороне.
    for (const angle of [0, preferred, preferred + Math.PI / 2, preferred - Math.PI / 2]) {
      if (boxFits(points, cx, cy, halfWidth, halfHeight, angle)) return angle;
    }
    return preferred;
  }

  function faceNormal(shape, faceIndex) {
    const indices = shape.faces[faceIndex];
    const points = indices.map((index) => shape.vertices[index]);
    let normal = normalize(cross(subtract(points[1], points[0]), subtract(points[2], points[0])));
    const centroid = points.reduce(
      (sum, point) => [sum[0] + point[0], sum[1] + point[1], sum[2] + point[2]],
      [0, 0, 0]
    ).map((value) => value / points.length);
    if (dot(normal, centroid) < 0) normal = normal.map((value) => -value);
    return normal;
  }

  /**
   * Поворот, при котором выбранная грань смотрит прямо на игрока.
   * Сначала нормаль грани приводится к +Z, затем грань доворачивается
   * вокруг Z так, чтобы её длинное ребро легло горизонтально.
   */
  function finalOrientation(shape, faceIndex) {
    const indices = shape.faces[faceIndex];
    const points = indices.map((index) => shape.vertices[index]);
    const normal = faceNormal(shape, faceIndex);
    const ay = Math.atan2(-normal[0], normal[2]);
    const ax = Math.atan2(normal[1], Math.hypot(normal[0], normal[2]));
    const flat = points.map((point) => rotate(point, ax, ay, 0));
    let edge = null;
    let longest = 0;
    for (let index = 0; index < flat.length; index += 1) {
      const a = flat[index];
      const b = flat[(index + 1) % flat.length];
      const dx = b[0] - a[0];
      const dy = b[1] - a[1];
      const length = dx * dx + dy * dy;
      if (length > longest) {
        longest = length;
        edge = [dx, dy];
      }
    }
    let az = 0;
    if (edge) {
      // Ведём ребро слева направо, чтобы цифра не оказалась вверх ногами.
      const dx = edge[0] < 0 ? -edge[0] : edge[0];
      const dy = edge[0] < 0 ? -edge[1] : edge[1];
      az = -Math.atan2(dy, dx);
    }
    return { ax, ay, az };
  }

  function drawValue(context, face, size, value, alpha = 1) {
    const points = face.projected;
    const text = String(value);
    if (!text || points.length < 3 || face.inradius < 2) return;
    const fontSize = Math.min(
      size * 0.28,
      face.inradius * 1.25 / (text.length > 1 ? 1.15 : 1)
    );
    if (fontSize < 5) return;
    const halfWidth = fontSize * 0.32 * text.length;
    const halfHeight = fontSize * 0.6;
    const angle = bestAngle(points, face.cx, face.cy, halfWidth, halfHeight, face.edgeAngle);

    context.save();
    context.globalAlpha = alpha;
    context.beginPath();
    points.forEach(([x, y], index) => {
      if (index === 0) context.moveTo(x, y);
      else context.lineTo(x, y);
    });
    context.closePath();
    context.clip();
    context.translate(face.cx, face.cy);
    context.rotate(angle);
    context.font = `700 ${fontSize}px "Cinzel", Georgia, serif`;
    context.textAlign = 'center';
    context.textBaseline = 'middle';
    context.fillStyle = '#2a1a0d';
    context.fillText(text, 0, 0);
    context.restore();
  }

  function draw(context, shape, size, ax, ay, az, value, showValue, target, lift = 0, valueAlpha = 1) {
    context.clearRect(0, 0, size, size);

    const glow = context.createRadialGradient(
      size / 2, size / 2, size * 0.12,
      size / 2, size / 2, size * 0.54
    );
    glow.addColorStop(0, 'rgba(231, 203, 146, 0.22)');
    glow.addColorStop(1, 'rgba(231, 203, 146, 0)');
    context.fillStyle = glow;
    context.fillRect(0, 0, size, size);

    // Тень на столе: пока кость в воздухе, она шире и мягче.
    const air = Math.max(0, Math.min(1, lift));
    context.save();
    context.translate(size / 2, size * 0.88);
    context.scale(1, 0.28);
    const shadowRadius = size * (0.30 + 0.14 * air);
    const shadow = context.createRadialGradient(0, 0, 0, 0, 0, shadowRadius);
    shadow.addColorStop(0, `rgba(0, 0, 0, ${0.46 * (1 - 0.55 * air)})`);
    shadow.addColorStop(1, 'rgba(0, 0, 0, 0)');
    context.fillStyle = shadow;
    context.beginPath();
    context.arc(0, 0, shadowRadius, 0, Math.PI * 2);
    context.fill();
    context.restore();

    const rotated = shape.vertices.map((vertex) => rotate(vertex, ax, ay, az));
    const faces = shape.faces.map((indices, index) => {
      const points = indices.map((vertexIndex) => rotated[vertexIndex]);
      let normal = normalize(cross(subtract(points[1], points[0]), subtract(points[2], points[0])));
      const centroid = points.reduce(
        (sum, point) => [sum[0] + point[0], sum[1] + point[1], sum[2] + point[2]],
        [0, 0, 0]
      ).map((value) => value / points.length);
      // Наружу, а не внутрь: иначе передние грани получают тень, и кость
      // выглядит тёмным силуэтом с золотым ободком.
      if (dot(normal, centroid) < 0) normal = normal.map((value) => -value);
      const depth = points.reduce((sum, point) => sum + point[2], 0) / points.length;
      return { index, points, normal, depth };
    }).sort((left, right) => left.depth - right.depth);

    const line = Math.max(1, size * 0.014);
    for (const face of faces) {
      const projected = face.points.map((point) => project(point, size, lift));
      face.projected = projected;
      face.area = polygonArea(projected);
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

    if (!showValue || !value) return;
    const front = faces.find((face) => face.index === target);
    if (!front || !front.projected) return;
    const cx = front.projected.reduce((sum, point) => sum + point[0], 0) / front.projected.length;
    const cy = front.projected.reduce((sum, point) => sum + point[1], 0) / front.projected.length;
    front.cx = cx;
    front.cy = cy;
    front.edgeAngle = uprightAngle(front.projected, cx, cy);
    front.inradius = Math.min(
      polygonInradius(front.projected, cx, cy),
      polygonArea(front.projected) / 8
    );
    drawValue(context, front, size, value, valueAlpha);
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

    const context = canvas.getContext('2d');
    if (!context) return null;
    context.setTransform(ratio, 0, 0, ratio, 0, 0);

    const started = performance.now();
    const target = 0;
    const final = finalOrientation(shape, target);
    // Лёгкий наклон: грань остаётся к игроку, но у куба и тетраэдра
    // появляется глубина, а не фронтальный квадрат.
    const finalX = final.ax + (mini ? 0.06 : 0.14);
    const finalY = final.ay + (mini ? 0.08 : 0.2);
    const finalZ = final.az;
    const settleTime = mini ? 0.26 : 0.42;

    // Кость подбрасывается, летит по гравитации, отскакивает и теряет
    // вращение от трения о стол. Начальная закрутка случайна по оси, поэтому
    // каждый бросок падает по-своему.
    const GRAVITY = 18;
    const RESTITUTION = 0.45;
    const AIR_DRAG = 0.8;
    const TABLE_DRAG = 7;
    const state = {
      ax: 0,
      ay: 0,
      az: 0,
      height: 0,
      velocity: mini ? 2.7 : 3.2,
      wx: (Math.random() * 2 - 1) * 13,
      wy: (Math.random() * 2 - 1) * 15,
      wz: (Math.random() * 2 - 1) * 8
    };
    let last = started;
    let restAt = 0;
    let frame = 0;

    // Кратчайший поворот между углами, чтобы доводка не крутила лишний оборот.
    const shortest = (from, to) => {
      let delta = (to - from) % (Math.PI * 2);
      if (delta > Math.PI) delta -= Math.PI * 2;
      if (delta < -Math.PI) delta += Math.PI * 2;
      return delta;
    };

    const tick = (now) => {
      if (!host.isConnected) return;
      const elapsed = (now - started) / 1000;
      let remaining = Math.min(0.05, (now - last) / 1000);
      last = now;

      while (remaining > 0) {
        const step = Math.min(remaining, 1 / 120);
        remaining -= step;
        const air = state.height > 0.001 || state.velocity > 0;
        const decay = Math.exp(-(air ? AIR_DRAG : TABLE_DRAG) * step);
        state.wx *= decay;
        state.wy *= decay;
        state.wz *= decay;
        state.ax += state.wx * step;
        state.ay += state.wy * step;
        state.az += state.wz * step;

        state.velocity -= GRAVITY * step;
        state.height += state.velocity * step;
        if (state.height <= 0) {
          state.height = 0;
          if (state.velocity < -0.7) {
            // Удар: скорость вверх и вращение гасятся столом.
            state.velocity = -state.velocity * RESTITUTION;
            state.wx *= 0.5;
            state.wy *= 0.5;
            state.wz *= 0.5;
          } else {
            state.velocity = 0;
            if (!restAt) restAt = elapsed;
          }
        }
      }

      // Если окно было свёрнуто и кадры не шли, не ждём физику вечно:
      // доводим кость к грани по времени.
      if (!restAt && elapsed > (mini ? 1.1 : 1.6)) restAt = elapsed;

      // Доводка к выигравшей грани идёт как трение: быстро в начале,
      // плавно в конце.
      const settle = restAt ? Math.min(1, (elapsed - restAt) / settleTime) : 0;
      const eased = 1 - (1 - settle) ** 3;
      const ax = state.ax + shortest(state.ax, finalX) * eased;
      const ay = state.ay + shortest(state.ay, finalY) * eased;
      const az = state.az + shortest(state.az, finalZ) * eased;
      // Число проступает после остановки, а не вспыхивает.
      const fade = restAt
        ? Math.min(1, Math.max(0, (elapsed - restAt - settleTime) / 0.18))
        : 0;
      const showValue = !mini && Boolean(value) && fade > 0;
      draw(context, shape, size, ax, ay, az, value, showValue, target, state.height, fade);

      if (fade < 1 && elapsed < 6) {
        frame = requestAnimationFrame(tick);
      }
    };
    frame = requestAnimationFrame(tick);

    return () => cancelAnimationFrame(frame);
  }

  global.DnDDice3D = { mount };
}(window));
