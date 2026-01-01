// --- CONFIGURATION ---
const WS_URL = (location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/ws';
const SAMPLE_RATE = 24000;
const VAD_THRESHOLD = 0.02; 
const SILENCE_TIMEOUT = 2000; // 2 Saniye sessizlik (Daha doğal konuşma için)

// --- STATE ---
let socket;
let audioCtx;
let source, processor;
let isRec = false;      // Mikrofon açık mı?
let aiSpeaking = false; // AI şu an konuşuyor mu?
let handsFree = true;   // Sürekli dinleme modu

let silenceTimer = null;
let currentAiBubble = null;
let audioQueue = [];
let isPlayingQueue = false;

// --- DOM ---
const micBtn = document.getElementById('micBtn');
const micRipple = document.getElementById('micRipple');
const statusText = document.getElementById('statusText');
const connStatus = document.getElementById('connStatus');
const messages = document.getElementById('messages');
const logs = document.getElementById('logs');
const handsFreeBtn = document.getElementById('handsFreeBtn');

// --- INIT ---
function init() {
    connectWS();
    
    micBtn.onclick = toggleMic;
    handsFreeBtn.onclick = () => {
        handsFree = !handsFree;
        handsFreeBtn.classList.toggle('active', handsFree);
    };

    // Audio Unlock
    document.body.addEventListener('click', () => {
        if (!audioCtx) audioCtx = new (window.AudioContext || window.webkitAudioContext)({ sampleRate: SAMPLE_RATE });
        if (audioCtx.state === 'suspended') audioCtx.resume();
    }, { once: true });
}

function connectWS() {
    socket = new WebSocket(WS_URL);
    socket.binaryType = 'arraybuffer';

    socket.onopen = () => {
        connStatus.classList.add('online');
        micBtn.disabled = false;
        addLog("SYS", "Gateway connected");
        statusText.innerText = "Hazır";
    };

    socket.onclose = () => {
        connStatus.classList.remove('online');
        micBtn.disabled = true;
        statusText.innerText = "Bağlantı Koptu";
        setTimeout(connectWS, 3000);
    };

    socket.onmessage = async (e) => {
        if (e.data instanceof ArrayBuffer) {
            // AUDIO CHUNK
            queueAudio(e.data);
        } else {
            // TEXT DATA
            try {
                const msg = JSON.parse(e.data);
                if (msg.type === 'telemetry') {
                    addLog(msg.phase.toUpperCase(), msg.detail);
                } else if (msg.type === 'subtitle') {
                    handleSubtitle(msg.text);
                }
            } catch (err) { console.error(err); }
        }
    };
}

// --- SUBTITLE HANDLER ---
function handleSubtitle(text) {
    // "Thinking" animasyonunu kaldır
    const thinking = document.getElementById('thinking-bubble');
    if (thinking) thinking.remove();

    if (!currentAiBubble) {
        currentAiBubble = addMsg('ai', '');
    }
    
    // Metni ekle
    const textSpan = currentAiBubble.querySelector('.text');
    textSpan.innerText += text;
    
    // Otomatik kaydır
    messages.scrollTop = messages.scrollHeight;
}

// --- AUDIO ENGINE (QUEUE SYSTEM) ---
function queueAudio(data) {
    audioQueue.push(data);
    if (!isPlayingQueue) playNextInQueue();
}

function playNextInQueue() {
    if (audioQueue.length === 0) {
        isPlayingQueue = false;
        // AI Sustu -> Mikrofonu Dinlemeye Devam Et (Eğer HandsFree ise)
        setTimeout(() => {
            aiSpeaking = false;
            currentAiBubble = null; // Yeni cümle için balonu sıfırla
            if(isRec) {
                statusText.innerText = "Dinliyor...";
                micRipple.classList.add('listening');
            }
        }, 500); // 500ms bekleme (yankı bitişi için)
        return;
    }

    isPlayingQueue = true;
    aiSpeaking = true; // AI Konuşuyor -> Mikrofonu Yazılımsal Kapat
    
    // UI Güncelleme
    statusText.innerText = "Sentiric Konuşuyor...";
    micRipple.classList.remove('listening');

    if (!audioCtx) audioCtx = new (window.AudioContext || window.webkitAudioContext)({ sampleRate: SAMPLE_RATE });

    const data = audioQueue.shift();
    const i16 = new Int16Array(data);
    const f32 = new Float32Array(i16.length);
    for(let i=0; i<i16.length; i++) f32[i] = i16[i] / 32768.0;

    const buf = audioCtx.createBuffer(1, f32.length, SAMPLE_RATE);
    buf.getChannelData(0).set(f32);
    
    const src = audioCtx.createBufferSource();
    src.buffer = buf;
    src.connect(audioCtx.destination);
    src.start(0);
    
    src.onended = () => {
        playNextInQueue();
    };
}

// --- MICROPHONE LOGIC ---
async function toggleMic() {
    if (isRec) {
        stopMic();
    } else {
        await startMic();
    }
}

async function startMic() {
    try {
        if (!audioCtx) audioCtx = new (window.AudioContext || window.webkitAudioContext)({ sampleRate: SAMPLE_RATE });
        await audioCtx.resume();
        
        const stream = await navigator.mediaDevices.getUserMedia({ 
            audio: { 
                echoCancellation: true, 
                noiseSuppression: true, 
                autoGainControl: true,
                channelCount: 1
            } 
        });
        
        source = audioCtx.createMediaStreamSource(stream);
        processor = audioCtx.createScriptProcessor(4096, 1, 1);
        
        source.connect(processor);
        processor.connect(audioCtx.destination);

        processor.onaudioprocess = (e) => {
            // ECHO CANCELLATION (CRITICAL): AI konuşurken mikrofon verisini çöpe at
            if (aiSpeaking) return;

            const input = e.inputBuffer.getChannelData(0);
            
            // VAD Logic
            let sum = 0;
            for(let i=0; i<input.length; i++) sum += input[i]*input[i];
            const rms = Math.sqrt(sum / input.length);
            
            if (rms > VAD_THRESHOLD) {
                // Konuşma Algılandı
                statusText.innerText = "Ses Algılanıyor...";
                micRipple.classList.add('listening');
                
                clearTimeout(silenceTimer);
                silenceTimer = setTimeout(() => {
                    // Sessizlik Doldu -> Gönderimi Tamamla
                    if (handsFree) {
                        statusText.innerText = "Düşünüyor...";
                        micRipple.classList.remove('listening');
                        addThinkingBubble(); // UI'da göster
                    } else {
                        stopMic();
                    }
                }, SILENCE_TIMEOUT);

                // Veriyi Gönder
                sendAudio(input);
            } else {
                // Sessizlik anında status güncelleme (Sadece konuşma yoksa)
                 if (!aiSpeaking && statusText.innerText === "Ses Algılanıyor...") {
                    // statusText.innerText = "Dinliyor...";
                 }
            }
        };
        
        isRec = true;
        micBtn.classList.add('active');
        statusText.innerText = "Dinliyor...";
        micRipple.classList.add('listening');
        
    } catch (e) {
        alert("Mikrofon hatası: " + e.message);
    }
}

function stopMic() {
    if (processor) { processor.disconnect(); processor = null; }
    if (source) { source.mediaStream.getTracks().forEach(t => t.stop()); source.disconnect(); source = null; }
    isRec = false;
    micBtn.classList.remove('active');
    micRipple.classList.remove('listening');
    statusText.innerText = "Hazır";
}

function sendAudio(f32) {
    if (socket.readyState !== 1) return;
    const i16 = new Int16Array(f32.length);
    for(let i=0; i<f32.length; i++) {
        let s = Math.max(-1, Math.min(1, f32[i]));
        i16[i] = s < 0 ? s * 0x8000 : s * 0x7FFF;
    }
    socket.send(i16.buffer);
}

// --- UI HELPERS ---
function addThinkingBubble() {
    const existing = document.getElementById('thinking-bubble');
    if (existing) return;

    const div = document.createElement('div');
    div.id = 'thinking-bubble';
    div.className = 'msg-row ai';
    div.innerHTML = `
        <div class="avatar"><i class="fas fa-robot"></i></div>
        <div class="bubble">
            <div class="typing"><div class="dot"></div><div class="dot"></div><div class="dot"></div></div>
        </div>
    `;
    messages.appendChild(div);
    messages.scrollTop = messages.scrollHeight;
}

function addMsg(role, text) {
    const div = document.createElement('div');
    div.className = `msg-row ${role}`;
    const icon = role === 'user' ? 'fa-user' : 'fa-robot';
    div.innerHTML = `
        <div class="avatar"><i class="fas ${icon}"></i></div>
        <div class="bubble"><span class="text">${text}</span></div>
    `;
    messages.appendChild(div);
    messages.scrollTop = messages.scrollHeight;
    return div.querySelector('.bubble');
}

function addLog(tag, msg) {
    const d = document.createElement('div');
    d.className = 'log-entry';
    d.innerHTML = `<span class="tag ${tag.toLowerCase()}">[${tag}]</span> ${msg}`;
    logs.prepend(d);
}

init();