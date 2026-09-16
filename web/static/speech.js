(() => {
  const card = document.querySelector('#speechDemo')
  if (!card) return

  const pill = card.querySelector('.speech-pill')
  const dots = [...pill.querySelectorAll('.speech-dots span')]
  const label = pill.querySelector('.speech-label')
  const wash = pill.querySelector('.speech-wash')
  const blobs = [...wash.children]
  const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)')
  const styles = getComputedStyle(document.documentElement)
  const color = token => styles.getPropertyValue(token).trim()
  const colors = [color('--signal-red'), '#ffffff', color('--signal-magenta')]
  const rgb = hex => hex.slice(1).match(/../g).map(value => parseInt(value, 16))
  const clamp = value => Math.max(0, Math.min(1, value))
  let frame = 0
  let started = 0
  let lastTick = 0
  let levels = Array(15).fill(0)
  let liveColor = rgb(colors[0])
  let glowLevels = [0, 0, 0]

  // The phase targets and 33 ms ballistics follow crates/diktafon/src/pill.rs.
  function twinkle(col, row, time) {
    const step = time * 2.5
    const beat = Math.floor(step)
    const fraction = step - beat
    const random = key => {
      const value = Math.sin((col * 7.3 + row * 13.7) * 127.1 + key * 311.7) * 43758.547
      return value - Math.floor(value)
    }
    const a = random(beat) > 0.7 ? 1 : 0
    const b = random(beat + 1) > 0.7 ? 1 : 0
    const ease = fraction < 0.5 ? fraction * 2 : (1 - fraction) * 2
    return Math.max(a * (1 - fraction), b * fraction) * (0.55 + 0.45 * ease)
  }

  function voiceBand(col, time) {
    const phrase = 0.35 + 0.65 * Math.abs(Math.sin(time * 2.3))
    const spectrum = [0.8, 0.65, 0.5, 0.38, 0.28][col]
    return clamp(phrase * spectrum + Math.sin(time * 8 + col * 1.7) * 0.16)
  }

  function dotTarget(col, row, time) {
    if (time < 4) return clamp(voiceBand(col, time) * 3.6 - (2 - row))
    if (time < 6) {
      const scan = (time * 4) % 7 - 1
      return Math.max(0, 1 - Math.abs(col - scan) / 1.5) * 0.9
    }
    if (time < 8) return twinkle(col, row, time)
    return clamp(Math.min(1, (time - 8) / 0.38) * 4.4 - (2 - row))
  }

  function render(time) {
    const recording = time < 4
    const phase = recording ? 'recording' : time < 6 ? 'transcribing' : 'polishing'
    pill.dataset.phase = phase
    label.textContent = recording ? `0:0${Math.floor(time)}` : time < 6 ? 'Transcribing' : 'Polishing'
    const targetColor = rgb(recording ? colors[0] : time < 6 || time >= 8 ? colors[1] : colors[2])
    liveColor = liveColor.map((value, i) => value + (targetColor[i] - value) * 0.22)
    const channels = liveColor.map(Math.round).join(' ')
    dots.forEach((dot, i) => {
      const target = dotTarget(i % 5, Math.floor(i / 5), time)
      levels[i] += (target - levels[i]) * (target > levels[i] ? 0.55 : 0.14)
      const lit = levels[i]
      dot.style.backgroundColor = `rgb(${channels} / ${0.24 + lit * 0.76})`
      dot.style.opacity = '1'
      dot.style.boxShadow = lit > 0.3 ? `0 0 ${4 + lit * 6}px rgb(${channels} / ${lit * 242 / 255})` : 'none'
    })
    wash.style.opacity = recording ? '0.4' : '0'
    blobs.forEach((blob, i) => {
      const target = voiceBand(i * 2, time)
      glowLevels[i] += (target - glowLevels[i]) * (target > glowLevels[i] ? 0.55 : 0.08)
      blob.style.left = `${[30, 85, 140][i] + Math.sin(time * 0.4 + i * 2.1) * 16 - 4}px`
      blob.style.opacity = `${Math.min(1, 0.22 + glowLevels[i]) * 170 / 255}`
    })
  }

  function rest() {
    cancelAnimationFrame(frame)
    frame = 0
    pill.removeAttribute('data-phase')
    label.textContent = 'Transcribing'
    wash.style.opacity = '0'
    dots.forEach(dot => dot.removeAttribute('style'))
  }

  function tick(now) {
    const elapsed = now - started
    if (elapsed >= 8750) {
      rest()
      return
    }
    if (now - lastTick >= 33) {
      render(elapsed / 1000)
      lastTick = now
    }
    frame = requestAnimationFrame(tick)
  }

  function play() {
    if (reducedMotion.matches || document.hidden) return
    cancelAnimationFrame(frame)
    levels = Array(15).fill(0)
    glowLevels = [0, 0, 0]
    liveColor = rgb(colors[0])
    started = performance.now()
    lastTick = started
    render(0)
    frame = requestAnimationFrame(tick)
  }

  card.addEventListener('pointerenter', play)
  card.addEventListener('pointerdown', () => { if (!frame) play() })
  reducedMotion.addEventListener('change', rest)
  document.addEventListener('visibilitychange', () => { if (document.hidden) rest() })
  const observer = new IntersectionObserver(entries => {
    if (entries[0].isIntersecting) play()
    else rest()
  }, { threshold: 0.5 })
  observer.observe(card)
})()
