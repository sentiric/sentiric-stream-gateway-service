/**
 * SENTIRIC AUDIO CORE ENGINE (v2.1 Stable)
 * Handles Microphone Input, Downsampling (48k->16k), and Jitter-Buffered Playback.
 */

// Global Audio Context State
window._audioContext = null;
window._mediaRecorder = null;
let analyser = null;
let processor = null; 
let audioWorkletNode = null; // Future proofing

// --- JITTER BUFFER & GAPLESS PLAYBACK STATE ---
let nextStartTime = 0; 
let isDownloadFinished = false;
let activeSourceNodes = []; 
let isStopRequested = false;

// --- BAŞLATMA TAMPONU (PRIMING BUFFER) ---
let primingBuffer = [];
let isPlaybackStarted = false;
const PRIMING_BUFFER_DURATION_MS = 600; // Latency vs Stability trade-off (Lowered to 600ms for responsiveness)

// --- DOWNSAMPLING WORKER LOGIC ---
// 48kHz/44.1kHz -> 16kHz dönüşümü yapar.
function downsampleBuffer(buffer, inputSampleRate, targetSampleRate) {
    if (targetSampleRate === inputSampleRate) {
        return buffer;
    }
    if (targetSampleRate > inputSampleRate) {
        console.warn("Upsampling not supported in this context.");
        return buffer;
    }
    
    const sampleRateRatio = inputSampleRate / targetSampleRate;
    const newLength = Math.round(buffer.length / sampleRateRatio);
    const result = new Float32Array(newLength);
    
    let offsetResult = 0;
    let offsetBuffer = 0;
    
    while (offsetResult < result.length) {
        const nextOffsetBuffer = Math.round((offsetResult + 1) * sampleRateRatio);
        // Use averaging for simple anti-aliasing approximation
        let accum = 0, count = 0;
        for (let i = offsetBuffer; i < nextOffsetBuffer && i < buffer.length; i++) {
            accum += buffer[i];
            count++;
        }
        
        result[offsetResult] = count > 0 ? accum / count : 0;
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
            analyser.smoothingTimeConstant = 0.5; // More responsive viz
            analyser.connect(window._audioContext.destination);
            initVisualizer();
            console.log(`[AudioCore] Context Initialized. System Rate: ${window._audioContext.sampleRate}Hz`);
        } catch (e) {
            console.error("[AudioCore] Failed to initialize AudioContext:", e);
            return;
        }
    }
    
    if (window._audioContext.state === 'suspended') { 
        window._audioContext.resume().catch(e => console.warn("[AudioCore] Resume failed:", e)); 
    }
}

// --- PLAYBACK LOGIC ---

function notifyDownloadFinished() {
    isDownloadFinished = true;
    checkIfPlaybackFinished();
}

function checkIfPlaybackFinished() {
    // Eğer indirme bitti ama henüz hiç çalmadıysak (kısa sesler), buffer'ı boşalt
    if (!isPlaybackStarted && isDownloadFinished && primingBuffer.length > 0) {
        // Genellikle TTS 24kHz döner, ancak header yoksa varsayılanı kullanırız
        _startPlaybackFromPrimingBuffer(24000); 
    }
    
    if (isDownloadFinished && activeSourceNodes.length === 0) {
        if (window.onAudioPlaybackComplete) {
            window.onAudioPlaybackComplete();
        }
    }
}

function _schedulePlayback(float32Array, sampleRate) {
    const ctx = window._audioContext;
    if (!ctx || ctx.state === 'closed') return;

    // Zaman senkronizasyonu: Eğer nextStartTime geçmişte kaldıysa, şu ana çek.
    if (nextStartTime < ctx.currentTime) {
        nextStartTime = ctx.currentTime + 0.05; // 50ms güvenlik payı
    }

    const buffer = ctx.createBuffer(1, float32Array.length, sampleRate);
    buffer.getChannelData(0).set(float32Array);
    
    const source = ctx.createBufferSource();
    source.buffer = buffer;
    source.connect(analyser); // Görselleştiriciye bağla
    
    source.start(nextStartTime);
    nextStartTime += buffer.duration;
    
    activeSourceNodes.push(source);
    
    source.onended = () => {
        const index = activeSourceNodes.indexOf(source);
        if (index > -1) activeSourceNodes.splice(index, 1);
        // Garbage collection'a yardımcı ol
        source.disconnect();
        checkIfPlaybackFinished();
    };
}

function _startPlaybackFromPrimingBuffer(sampleRate) {
    const ctx = window._audioContext;
    if (primingBuffer.length === 0 || !ctx) return;
    
    // Bufferları birleştir
    const totalLength = primingBuffer.reduce((sum, arr) => sum + arr.length, 0);
    const concatenatedBuffer = new Float32Array(totalLength);
    let offset = 0;
    for (const chunk of primingBuffer) {
        concatenatedBuffer.set(chunk, offset);
        offset += chunk.length;
    }
    
    // İlk oynatma
    const currentTime = ctx.currentTime;
    const WAKE_UP_DELAY = 0.1; 
    nextStartTime = currentTime + WAKE_UP_DELAY;
    
    _schedulePlayback(concatenatedBuffer, sampleRate);
    primingBuffer = []; // Temizle
}

async function playChunk(float32Array, sampleRate) {
    initAudioContext();
    if (isStopRequested) return;

    if (!isPlaybackStarted) {
        primingBuffer.push(float32Array);
        const currentBufferedDuration = (primingBuffer.reduce((sum, arr) => sum + arr.length, 0) / sampleRate) * 1000;

        if (currentBufferedDuration >= PRIMING_BUFFER_DURATION_MS || isDownloadFinished) {
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
    
    // Aktif sesleri nazikçe durdur
    activeSourceNodes.forEach(node => { 
        try { 
            node.stop(0); 
            node.disconnect(); 
        } catch(e) {} 
    });
    activeSourceNodes = [];
    nextStartTime = 0;
    
    primingBuffer = [];
    isPlaybackStarted = false;
    
    // Mic stream'i de temizle
    stopMicStream();
    
    // Flag'i sıfırla
    setTimeout(() => { isStopRequested = false; }, 100);
}

// --- VISUALIZER ---
function initVisualizer() {
    const canvas = document.getElementById('visualizer');
    if(!canvas) return; // UI'da canvas yoksa hata verme
    
    const ctx = canvas.getContext('2d');
    function resize() { 
        if(canvas.parentElement) {
            canvas.width = canvas.parentElement.offsetWidth; 
            canvas.height = canvas.parentElement.offsetHeight; 
        }
    }
    window.addEventListener('resize', resize); 
    resize();

    function draw() {
        requestAnimationFrame(draw);
        if(!analyser) return;
        
        const bufferLength = analyser.frequencyBinCount;
        const dataArray = new Uint8Array(bufferLength);
        analyser.getByteFrequencyData(dataArray);
        
        ctx.clearRect(0, 0, canvas.width, canvas.height);
        
        // Bar genişliği ve aralık
        const barWidth = (canvas.width / bufferLength) * 2.5;
        let x = 0;
        
        for(let i = 0; i < bufferLength; i++) {
            const v = dataArray[i] / 255.0;
            const h = v * canvas.height;
            
            // Dinamik renk (Ses şiddetine göre)
            const hue = 220; // Mavi tonu
            const lightness = 50 + (v * 20);
            ctx.fillStyle = `hsl(${hue}, 80%, ${lightness}%)`;
            
            ctx.fillRect(x, canvas.height - h, barWidth, h);
            x += barWidth + 1;
        }
    }
    draw();
}

// --- MICROPHONE STREAMING (INPUT) ---
async function startMicStream(onDataCallback) {
    initAudioContext();
    if (!navigator.mediaDevices) return alert("Mikrofon erişimi desteklenmiyor veya engellendi.");
    
    try {
        // İnsan sesi için optimize edilmiş constraintler
        const stream = await navigator.mediaDevices.getUserMedia({ 
            audio: {
                channelCount: 1,
                echoCancellation: true,
                noiseSuppression: true,
                autoGainControl: true,
                sampleRate: 48000 // Tercihen yüksek alıp kendimiz düşürelim
            } 
        });
        
        const ctx = window._audioContext;
        window.mediaStreamSource = ctx.createMediaStreamSource(stream);
        
        // ScriptProcessorNode (Buffer Size: 4096 = ~92ms gecikme @ 44.1kHz)
        // Production notu: Gelecekte AudioWorklet'e taşınmalı.
        processor = ctx.createScriptProcessor(4096, 1, 1);
        
        window.mediaStreamSource.connect(processor);
        processor.connect(ctx.destination);
        
        processor.onaudioprocess = (e) => {
            if (isStopRequested) return;

            const inputData = e.inputBuffer.getChannelData(0);
            
            // 1. DOWNSAMPLE (System Rate -> 16000 Hz)
            const targetRate = 16000;
            const resampled = downsampleBuffer(inputData, ctx.sampleRate, targetRate);
            
            // 2. CONVERT TO INT16 (PCM)
            const pcm16 = new Int16Array(resampled.length);
            for (let i = 0; i < resampled.length; i++) {
                // Hard clipping (-1.0 to 1.0)
                let s = Math.max(-1, Math.min(1, resampled[i]));
                // Float to Int16 mapping
                pcm16[i] = s < 0 ? s * 0x8000 : s * 0x7FFF;
            }
            
            // 3. CALLBACK (WebSocket Send)
            if(onDataCallback) onDataCallback(pcm16.buffer);
        };
        
        console.log(`[AudioCore] Mic stream started. Resampling ${ctx.sampleRate}Hz -> 16000Hz`);
        
    } catch(e) {
        console.error("[AudioCore] Mic Error:", e);
        alert("Mikrofon başlatılamadı: " + e.message);
    }
}

function stopMicStream() {
    if (processor) {
        processor.disconnect();
        processor.onaudioprocess = null;
        processor = null;
    }
    if (window.mediaStreamSource) {
        window.mediaStreamSource.stream.getTracks().forEach(track => track.stop());
        window.mediaStreamSource.disconnect();
        window.mediaStreamSource = null;
    }
}

// Global Export
window.AudioCore = {
    init: initAudioContext,
    startMicStream: startMicStream,
    stopMicStream: stopMicStream,
    playChunk: playChunk,
    reset: resetAudioState
};