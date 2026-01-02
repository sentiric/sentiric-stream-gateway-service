// Global Audio Context
window._audioContext = null;
window._mediaRecorder = null;
let analyser = null;
let processor = null; // ScriptProcessor yerine AudioWorklet tercih edilse de, hızlı fix için ScriptProcessor

// --- JITTER BUFFER & GAPLESS PLAYBACK STATE ---
let nextStartTime = 0; 
let isDownloadFinished = false;
let activeSourceNodes = []; 
let isStopRequested = false;

// --- BAŞLATMA TAMPONU (PRIMING BUFFER) ---
let primingBuffer = [];
let isPlaybackStarted = false;
var PRIMING_BUFFER_DURATION_MS = 1000;

// --- DOWNSAMPLING WORKER (Inline for simplicity) ---
// 48kHz/44.1kHz -> 16kHz dönüşümü yapar.
function downsampleBuffer(buffer, inputSampleRate, targetSampleRate) {
    if (targetSampleRate === inputSampleRate) {
        return buffer;
    }
    var sampleRateRatio = inputSampleRate / targetSampleRate;
    var newLength = Math.round(buffer.length / sampleRateRatio);
    var result = new Float32Array(newLength);
    var offsetResult = 0;
    var offsetBuffer = 0;
    while (offsetResult < result.length) {
        var nextOffsetBuffer = Math.round((offsetResult + 1) * sampleRateRatio);
        var accum = 0, count = 0;
        for (var i = offsetBuffer; i < nextOffsetBuffer && i < buffer.length; i++) {
            accum += buffer[i];
            count++;
        }
        result[offsetResult] = accum / count;
        offsetResult++;
        offsetBuffer = nextOffsetBuffer;
    }
    return result;
}

function initAudioContext() {
    if (!window._audioContext || window._audioContext.state === 'closed') {
        try {
            window._audioContext = new (window.AudioContext || window.webkitAudioContext)();
            analyser = window._audioContext.createAnalyser();
            analyser.fftSize = 256;
            analyser.smoothingTimeConstant = 0.8;
            analyser.connect(window._audioContext.destination);
            initVisualizer();
            console.log("AudioContext Initialized. System Rate:", window._audioContext.sampleRate);
        } catch (e) {
            console.error("Failed to initialize AudioContext:", e);
            return;
        }
    }
    
    if (window._audioContext.state === 'suspended') { 
        window._audioContext.resume().catch(e => console.warn("AudioContext resume failed:", e)); 
    }
}

// ... Playback fonksiyonları (değişmedi) ...
function notifyDownloadFinished() {
    isDownloadFinished = true;
    checkIfPlaybackFinished();
}

function checkIfPlaybackFinished() {
    if (!isPlaybackStarted && isDownloadFinished && primingBuffer.length > 0) {
        _startPlaybackFromPrimingBuffer(24000); // TTS genelde 24k döner
    }
    if (isDownloadFinished && activeSourceNodes.length === 0) {
        if (window.onAudioPlaybackComplete) window.onAudioPlaybackComplete();
    }
}

function _schedulePlayback(float32Array, sampleRate) {
    const ctx = window._audioContext;
    if (!ctx || ctx.state === 'closed') return;
    const buffer = ctx.createBuffer(1, float32Array.length, sampleRate);
    buffer.getChannelData(0).set(float32Array);
    const source = ctx.createBufferSource();
    source.buffer = buffer;
    source.connect(analyser); 
    source.start(nextStartTime);
    nextStartTime += buffer.duration;
    activeSourceNodes.push(source);
    source.onended = () => {
        const index = activeSourceNodes.indexOf(source);
        if (index > -1) activeSourceNodes.splice(index, 1);
        source.disconnect();
        checkIfPlaybackFinished();
    };
}

function _startPlaybackFromPrimingBuffer(sampleRate) {
    const ctx = window._audioContext;
    if (primingBuffer.length === 0 || !ctx) return;
    const currentTime = ctx.currentTime;
    const WAKE_UP_DELAY = 0.1;
    nextStartTime = currentTime + WAKE_UP_DELAY;
    const totalLength = primingBuffer.reduce((sum, arr) => sum + arr.length, 0);
    const concatenatedBuffer = new Float32Array(totalLength);
    let offset = 0;
    for (const chunk of primingBuffer) {
        concatenatedBuffer.set(chunk, offset);
        offset += chunk.length;
    }
    _schedulePlayback(concatenatedBuffer, sampleRate);
    primingBuffer = [];
}

async function playChunk(float32Array, sampleRate) {
    initAudioContext();
    if (isStopRequested) return;
    if (!isPlaybackStarted) {
        primingBuffer.push(float32Array);
        const currentBufferedDuration = primingBuffer.reduce((sum, arr) => sum + arr.length, 0) / sampleRate * 1000;
        if (currentBufferedDuration >= PRIMING_BUFFER_DURATION_MS) {
            isPlaybackStarted = true;
            _startPlaybackFromPrimingBuffer(sampleRate);
        }
    } else {
        _schedulePlayback(float32Array, sampleRate);
    }
}

function resetAudioState() {
    isStopRequested = true;
    isDownloadFinished = false;
    activeSourceNodes.forEach(node => { try { node.stop(0); node.disconnect(); } catch(e) {} });
    activeSourceNodes = [];
    nextStartTime = 0;
    primingBuffer = [];
    isPlaybackStarted = false;
    if(processor) { try { processor.disconnect(); processor = null; } catch(e){} }
    if(window.mediaStreamSource) { try { window.mediaStreamSource.disconnect(); } catch(e){} }
    setTimeout(() => { isStopRequested = false; }, 100);
}

function initVisualizer() {
    const canvas = document.getElementById('visualizer');
    if(!canvas) return;
    const ctx = canvas.getContext('2d');
    function resize() { canvas.width = canvas.offsetWidth; canvas.height = canvas.offsetHeight; }
    window.addEventListener('resize', resize); resize();
    function draw() {
        requestAnimationFrame(draw);
        if(!analyser) return;
        const bufferLength = analyser.frequencyBinCount;
        const dataArray = new Uint8Array(bufferLength);
        analyser.getByteFrequencyData(dataArray);
        ctx.clearRect(0, 0, canvas.width, canvas.height);
        const barWidth = (canvas.width / bufferLength) * 2.5;
        let x = 0;
        for(let i = 0; i < bufferLength; i++) {
            const v = dataArray[i] / 255.0;
            const h = v * canvas.height;
            ctx.fillStyle = `rgba(60, 130, 246, ${v + 0.2})`;
            ctx.fillRect(x, canvas.height - h, barWidth, h);
            x += barWidth + 1;
        }
    }
    draw();
}

// --- MIC RECORDING FOR VAD/STREAMING ---
async function startMicStream(onDataCallback) {
    initAudioContext();
    if (!navigator.mediaDevices) return alert("Mic denied");
    
    try {
        const stream = await navigator.mediaDevices.getUserMedia({ 
            audio: {
                channelCount: 1,
                echoCancellation: true,
                noiseSuppression: true,
                autoGainControl: true
            } 
        });
        
        const ctx = window._audioContext;
        window.mediaStreamSource = ctx.createMediaStreamSource(stream);
        
        // Buffer size 4096 gives ~92ms latency at 44.1kHz, acceptable.
        processor = ctx.createScriptProcessor(4096, 1, 1);
        
        window.mediaStreamSource.connect(processor);
        processor.connect(ctx.destination);
        
        processor.onaudioprocess = (e) => {
            const inputData = e.inputBuffer.getChannelData(0);
            
            // 1. DOWNSAMPLE (System Rate -> 16000)
            const targetRate = 16000;
            const resampled = downsampleBuffer(inputData, ctx.sampleRate, targetRate);
            
            // 2. CONVERT TO INT16
            // Bu format stream-gateway'in beklediği formattır.
            const pcm16 = new Int16Array(resampled.length);
            for (let i = 0; i < resampled.length; i++) {
                let s = Math.max(-1, Math.min(1, resampled[i]));
                pcm16[i] = s < 0 ? s * 0x8000 : s * 0x7FFF;
            }
            
            // 3. SEND
            if(onDataCallback) onDataCallback(pcm16.buffer);
        };
        
        console.log("Mic stream started with realtime resampling to 16kHz.");
        
    } catch(e) {
        console.error("Mic Error:", e);
        alert("Mikrofon hatası: " + e.message);
    }
}

function stopMicStream() {
    if (processor) {
        processor.disconnect();
        processor.onaudioprocess = null;
        processor = null;
    }
    if (window.mediaStreamSource) {
        window.mediaStreamSource.disconnect();
        window.mediaStreamSource = null;
    }
    console.log("Mic stream stopped.");
}

// Global export for app.js
window.AudioCore = {
    init: initAudioContext,
    startMicStream: startMicStream,
    stopMicStream: stopMicStream,
    playChunk: playChunk,
    reset: resetAudioState
};