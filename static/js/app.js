// --- CONFIGURATION ---
const WS_URL = (location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/ws';
const VAD_THRESHOLD = 0.02; // Sessizlik eşiği
const VAD_SILENCE_DURATION = 1000; // 1 saniye sessizlik sonrası gönderimi durdur

// --- STATE ---
let socket;
let audioContext;
let mediaStream;
let processor;
let isRecording = false;
let isSpeaking = false;
let silenceStart = Date.now();
let aiSpeaking = false; // Echo Cancellation için flag

// --- UI ELEMENTS ---
const micBtn = document.getElementById('micBtn');
const micText = document.getElementById('micText');
const chatArea = document.getElementById('chat-area');
const logArea = document.getElementById('log-area');
const statusBadge = document.getElementById('connStatus');

// --- INITIALIZATION ---
function init() {
    connectWebSocket();
    micBtn.onclick = toggleMicrophone;
}

function connectWebSocket() {
    socket = new WebSocket(WS_URL);
    
    socket.onopen = () => {
        statusBadge.textContent = "Connected";
        statusBadge.classList.add("connected");
        micBtn.disabled = false;
        addLog("gateway", "INFO", "WebSocket bağlantısı kuruldu.");
    };

    socket.onclose = () => {
        statusBadge.textContent = "Disconnected";
        statusBadge.classList.remove("connected");
        micBtn.disabled = true;
        setTimeout(connectWebSocket, 3000);
    };

    socket.onmessage = async (event) => {
        if (event.data instanceof Blob) {
            // AUDIO CHUNK (Binary) -> Play immediately
            if (!aiSpeaking) { 
                addLog("tts", "PLAY", "Audio chunk received (playing...)");
                aiSpeaking = true; // AI konuşmaya başladı, mikrofonu sustur (Echo Cancellation)
            }
            playAudioChunk(event.data);
        } else {
            // TEXT MESSAGE (JSON)
            try {
                const msg = JSON.parse(event.data);
                if (msg.type === 'telemetry') {
                    addLog(msg.phase, msg.status.toUpperCase(), msg.detail);
                } else if (msg.type === 'subtitle') {
                    addBubble("ai", msg.text);
                    aiSpeaking = true;
                }
            } catch (e) {
                console.error("Parse error:", e);
            }
        }
    };
}

// --- AUDIO PLAYBACK ---
async function playAudioChunk(blob) {
    if (!audioContext) audioContext = new (window.AudioContext || window.webkitAudioContext)();
    
    const arrayBuffer = await blob.arrayBuffer();
    const audioBuffer = await audioContext.decodeAudioData(arrayBuffer);
    
    const source = audioContext.createBufferSource();
    source.buffer = audioBuffer;
    source.connect(audioContext.destination);
    source.start(0);
    
    source.onended = () => {
        // Basit bir timeout ile AI'ın sustuğunu varsayıyoruz (Daha gelişmiş kuyruk yönetimi eklenebilir)
        setTimeout(() => { aiSpeaking = false; }, 500); 
    };
}

// --- MICROPHONE & VAD ---
async function toggleMicrophone() {
    if (isRecording) {
        stopMicrophone();
    } else {
        await startMicrophone();
    }
}

async function startMicrophone() {
    try {
        if (!audioContext) audioContext = new (window.AudioContext || window.webkitAudioContext)();
        if (audioContext.state === 'suspended') await audioContext.resume();

        mediaStream = await navigator.mediaDevices.getUserMedia({ audio: true });
        const source = audioContext.createMediaStreamSource(mediaStream);
        
        // Processor (AudioWorklet yerine ScriptProcessor kullanıyoruz - Legacy ama basit)
        processor = audioContext.createScriptProcessor(4096, 1, 1);
        
        source.connect(processor);
        processor.connect(audioContext.destination);

        processor.onaudioprocess = processAudio;
        
        isRecording = true;
        micBtn.classList.add('recording');
        micText.innerText = "Dinliyor...";
        addLog("client", "MIC", "Mikrofon aktif. VAD devrede.");
        
    } catch (e) {
        alert("Mikrofon hatası: " + e.message);
    }
}

function stopMicrophone() {
    if (processor) { processor.disconnect(); processor = null; }
    if (mediaStream) { mediaStream.getTracks().forEach(track => track.stop()); mediaStream = null; }
    isRecording = false;
    micBtn.classList.remove('recording');
    micText.innerText = "Başlat";
    addLog("client", "MIC", "Mikrofon kapatıldı.");
}

function processAudio(e) {
    if (aiSpeaking) return; // ECHO CANCELLATION: AI konuşurken dinleme

    const inputData = e.inputBuffer.getChannelData(0);
    
    // RMS Hesapla (Ses Şiddeti)
    let sum = 0;
    for (let i = 0; i < inputData.length; i++) sum += inputData[i] * inputData[i];
    const rms = Math.sqrt(sum / inputData.length);

    if (rms > VAD_THRESHOLD) {
        silenceStart = Date.now();
        if (!isSpeaking) {
            isSpeaking = true;
            addLog("client", "VAD", "Konuşma algılandı.");
        }
        // Sesi gönder (Float32 -> Int16 conversion needed for Rust?)
        // Rust tarafı binary raw PCM bekliyor olabilir veya Wav.
        // Basitlik için Float32 array'i gönderiyoruz (Rust tarafında dönüştürülmeli veya raw pcm)
        // Ancak WebSocket text/binary ayrımı önemli.
        // Hızlı çözüm: Doğrudan Int16'ya çevirip gönderelim.
        sendAudioChunk(inputData);
    } else {
        if (isSpeaking && (Date.now() - silenceStart > VAD_SILENCE_DURATION)) {
            isSpeaking = false;
            addLog("client", "VAD", "Sessizlik (Packet sonu).");
        }
    }
}

function sendAudioChunk(float32Data) {
    if (socket.readyState !== WebSocket.OPEN) return;
    
    // Float32 -> Int16 Conversion
    const int16Data = new Int16Array(float32Data.length);
    for (let i = 0; i < float32Data.length; i++) {
        const s = Math.max(-1, Math.min(1, float32Data[i]));
        int16Data[i] = s < 0 ? s * 0x8000 : s * 0x7FFF;
    }
    
    socket.send(int16Data.buffer); // Binary Frame
}

// --- UI HELPERS ---
function addBubble(type, text) {
    const div = document.createElement('div');
    div.className = `bubble ${type}`;
    div.innerHTML = `<span class="subtitle">${type === 'user' ? 'Siz' : 'Sentiric AI'}</span>${text}`;
    chatArea.appendChild(div);
    chatArea.scrollTop = chatArea.scrollHeight;
}

function addLog(phase, status, detail) {
    const div = document.createElement('div');
    let phaseClass = "";
    if (phase === 'stt') phaseClass = 'log-stt';
    if (phase === 'llm') phaseClass = 'log-llm';
    if (phase === 'tts') phaseClass = 'log-tts';
    
    div.className = `log-entry ${phaseClass}`;
    div.innerHTML = `<strong>[${phase.toUpperCase()}]</strong> ${status} - ${detail}`;
    logArea.appendChild(div);
    logArea.scrollTop = logArea.scrollHeight;
}

// Start
init();