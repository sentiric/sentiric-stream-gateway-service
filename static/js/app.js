/**
 * SENTIRIC OMNI-STUDIO v10.0 (GOLD MASTER)
 * Architecture: Singleton Class with Strict State Management
 */

const WORKLET_CODE = `
class VADProcessor extends AudioWorkletProcessor {
    constructor() { super(); this.bufferSize = 2048; this.buffer = new Float32Array(this.bufferSize); this.index = 0; }
    process(inputs) {
        const input = inputs[0];
        if (input && input.length > 0) {
            const channel = input[0];
            for (let i = 0; i < channel.length; i++) {
                this.buffer[this.index++] = channel[i];
                if (this.index >= this.bufferSize) { 
                    this.port.postMessage(this.buffer); 
                    this.index = 0; 
                }
            }
        }
        return true;
    }
}
registerProcessor('vad-processor', VADProcessor);
`;

class SentiricApp {
    constructor() {
        this.config = {
            wsUrl: (location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/ws',
            sampleRate: 24000,
            inputSampleRate: 16000,
            vadThreshold: 0.015,
            silenceDelay: 1500,
            minSpeechTime: 300
        };

        this.state = {
            socket: null,
            audioCtx: null,
            workletNode: null,
            isRecording: false,
            isUserSpeaking: false,
            aiSpeaking: false,
            silenceTimer: null,
            speechStartTime: 0,
            currentAiBubble: null,
            audioQueue: [],
            isPlayingQueue: false
        };

        this.ui = {
            splash: document.getElementById('splashScreen'),
            startBtn: document.getElementById('startBtn'),
            btn: document.getElementById('micBtn'),
            ring: document.getElementById('micRing'),
            status: document.getElementById('systemStatus'),
            dot: document.getElementById('connDot'),
            feed: document.getElementById('chatFeed'),
            canvas: document.getElementById('audioViz'),
            hint: document.querySelector('.hint')
        };
        
        // Canvas Context'i init'te alıyoruz ki referans hatası olmasın
        this.vizCtx = this.ui.canvas.getContext('2d');
        
        this.bindEvents();
    }

    bindEvents() {
        // 1. Splash Screen Click (AudioContext Unlock)
        this.ui.startBtn.onclick = () => {
            this.initAudioContext().then(() => {
                this.ui.splash.classList.add('hidden');
                this.connectWS();
            });
        };

        // 2. Mic Button Toggle
        this.ui.btn.onclick = () => this.toggleMic();

        // 3. Resize Canvas
        window.addEventListener('resize', () => this.resizeCanvas());
        this.resizeCanvas();
    }

    resizeCanvas() {
        this.ui.canvas.width = this.ui.canvas.parentElement.offsetWidth;
        this.ui.canvas.height = this.ui.canvas.parentElement.offsetHeight;
    }

    async initAudioContext() {
        try {
            this.state.audioCtx = new (window.AudioContext || window.webkitAudioContext)({ 
                sampleRate: this.config.sampleRate 
            });
            await this.state.audioCtx.resume();
            
            // Load Worklet
            const blob = new Blob([WORKLET_CODE], { type: 'application/javascript' });
            const url = URL.createObjectURL(blob);
            await this.state.audioCtx.audioWorklet.addModule(url);
            
            console.log("Audio Engine Initialized");
        } catch (e) {
            console.error("Audio Init Error:", e);
            alert("Ses sistemi başlatılamadı. Lütfen tarayıcı izinlerini kontrol edin.");
        }
    }

    async initMicrophone() {
        try {
            const stream = await navigator.mediaDevices.getUserMedia({ 
                audio: { 
                    echoCancellation: true, 
                    noiseSuppression: true, 
                    autoGainControl: true,
                    channelCount: 1,
                    sampleRate: this.config.inputSampleRate
                } 
            });

            const source = this.state.audioCtx.createMediaStreamSource(stream);
            this.state.workletNode = new AudioWorkletNode(this.state.audioCtx, 'vad-processor');
            
            source.connect(this.state.workletNode);
            // Binding 'this' explicitly
            this.state.workletNode.port.onmessage = (e) => this.processVad(e.data);
            
            return true;
        } catch (e) {
            console.error("Mic Error:", e);
            this.setStatus("Mikrofon Hatası", "error");
            return false;
        }
    }

    connectWS() {
        this.state.socket = new WebSocket(this.config.wsUrl);
        this.state.socket.binaryType = 'arraybuffer';

        this.state.socket.onopen = () => {
            this.ui.dot.classList.add('online');
            this.setStatus("Hazır", "neutral");
            this.ui.btn.disabled = false;
        };

        this.state.socket.onclose = () => {
            this.ui.dot.classList.remove('online');
            this.setStatus("Bağlanıyor...", "error");
            this.ui.btn.disabled = true;
            setTimeout(() => this.connectWS(), 3000);
        };

        this.state.socket.onmessage = (e) => this.handleMessage(e);
    }

    handleMessage(e) {
        if (e.data instanceof ArrayBuffer) {
            // Audio
            if (!this.state.aiSpeaking) {
                this.state.aiSpeaking = true;
                this.removeThinking();
                this.setStatus("Sentiric Konuşuyor", "speaking");
                this.ui.btn.className = "mic-btn speaking";
            }
            this.queueAudio(e.data);
        } else {
            // Text
            try {
                const msg = JSON.parse(e.data);
                if (msg.type === 'subtitle') {
                    this.removeThinking();
                    this.appendAiText(msg.text);
                } else if (msg.type === 'telemetry') {
                    if (msg.phase === 'stt' && msg.status === 'final') {
                        this.addUserMessage(msg.detail);
                        this.showThinking();
                        this.setStatus("Düşünüyor...", "processing");
                        this.ui.btn.className = "mic-btn processing";
                    }
                }
            } catch (err) {}
        }
    }

    processVad(float32Data) {
        if (!this.state.isRecording || this.state.aiSpeaking) {
            this.drawViz(0);
            return;
        }

        // RMS Calculation
        let sum = 0;
        for (let i = 0; i < float32Data.length; i++) sum += float32Data[i] * float32Data[i];
        const rms = Math.sqrt(sum / float32Data.length);
        
        this.drawViz(rms);

        if (rms > this.config.vadThreshold) {
            // Speech Detected
            if (!this.state.isUserSpeaking) {
                this.state.isUserSpeaking = true;
                this.state.speechStartTime = Date.now();
                this.setStatus("Dinliyor...", "listening");
                this.ui.btn.className = "mic-btn listening";
                this.ui.ring.classList.add('active');
            }
            
            if (this.state.silenceTimer) {
                clearTimeout(this.state.silenceTimer);
                this.state.silenceTimer = null;
            }
            
            this.sendSocketData(float32Data);
            
        } else {
            // Silence
            if (this.state.isUserSpeaking) {
                this.sendSocketData(float32Data); // Buffer tail

                if (!this.state.silenceTimer) {
                    this.state.silenceTimer = setTimeout(() => {
                        this.commitSpeech();
                    }, this.config.silenceDelay);
                }
            }
        }
    }

    commitSpeech() {
        this.state.isUserSpeaking = false;
        this.state.silenceTimer = null;
        
        this.ui.ring.classList.remove('active');
        
        const duration = Date.now() - this.state.speechStartTime;
        if (duration > this.config.minSpeechTime) {
            this.setStatus("İşleniyor...", "processing");
            this.ui.btn.className = "mic-btn processing";
        } else {
            this.setStatus("Dinliyor...", "neutral");
            this.ui.btn.className = "mic-btn active";
        }
    }

    sendSocketData(f32) {
        if (this.state.socket && this.state.socket.readyState === 1) {
            const i16 = new Int16Array(f32.length);
            for(let i=0; i<f32.length; i++) {
                let s = Math.max(-1, Math.min(1, f32[i]));
                i16[i] = s < 0 ? s * 0x8000 : s * 0x7FFF;
            }
            this.state.socket.send(i16.buffer);
        }
    }

    // --- AUDIO PLAYBACK ---
    queueAudio(data) {
        this.state.audioQueue.push(data);
        if (!this.state.isPlayingQueue) this.playNextAudio();
    }

    playNextAudio() {
        if (this.state.audioQueue.length === 0) {
            this.state.isPlayingQueue = false;
            setTimeout(() => {
                this.state.aiSpeaking = false;
                this.state.currentAiBubble = null;
                if (this.state.isRecording) {
                    this.setStatus("Dinliyor...", "neutral");
                    this.ui.btn.className = "mic-btn active";
                } else {
                    this.ui.btn.className = "mic-btn";
                }
            }, 500);
            return;
        }

        this.state.isPlayingQueue = true;
        const chunk = this.state.audioQueue.shift();
        
        if (!this.state.audioCtx) return;

        const i16 = new Int16Array(chunk);
        const f32 = new Float32Array(i16.length);
        for(let i=0; i<i16.length; i++) f32[i] = i16[i] / 32768.0;

        const buf = this.state.audioCtx.createBuffer(1, f32.length, this.config.sampleRate);
        buf.getChannelData(0).set(f32);
        const src = this.state.audioCtx.createBufferSource();
        src.buffer = buf;
        src.connect(this.state.audioCtx.destination);
        src.start(0);
        src.onended = () => this.playNextAudio();
    }

    // --- UI HELPERS ---
    async toggleMic() {
        if (!this.state.isRecording) {
            const success = await this.initMicrophone();
            if (success) {
                this.state.isRecording = true;
                this.ui.btn.classList.add('active');
                this.setStatus("Dinliyor...", "neutral");
            }
        } else {
            this.state.isRecording = false;
            this.state.workletNode.disconnect();
            this.ui.btn.className = "mic-btn";
            this.ui.ring.classList.remove('active');
            this.setStatus("Durduruldu", "neutral");
            this.drawViz(0);
        }
    }

    drawViz(rms) {
        const w = this.ui.canvas.width;
        const h = this.ui.canvas.height;
        this.vizCtx.clearRect(0, 0, w, h);
        
        if (rms === 0) return;

        const barHeight = Math.min(h, rms * 1500); // Scale factor
        const x = w / 2;
        const y = (h - barHeight) / 2;
        
        this.vizCtx.fillStyle = this.state.isUserSpeaking ? '#ef4444' : '#6366f1';
        this.vizCtx.beginPath();
        this.vizCtx.roundRect(x - 2, y, 4, barHeight, 2);
        this.vizCtx.fill();
        
        if (this.state.isUserSpeaking) {
             this.vizCtx.shadowBlur = 15;
             this.vizCtx.shadowColor = "#ef4444";
        } else {
             this.vizCtx.shadowBlur = 0;
        }
    }

    setStatus(text, type) {
        this.ui.status.innerText = text;
        this.ui.hint.innerText = type === 'listening' ? "Konuşma Algılandı" : 
                                 type === 'processing' ? "Yapay Zeka Yanıtlıyor" : 
                                 "Otomatik Algılama (VAD)";
        
        const colors = { 'listening': '#ef4444', 'processing': '#f59e0b', 'speaking': '#6366f1', 'neutral': '#71717a', 'error': '#ef4444' };
        this.ui.status.style.color = colors[type] || colors.neutral;
    }

    addUserMessage(text) {
        this.removeThinking();
        this.state.currentAiBubble = null;
        const div = document.createElement('div');
        div.className = 'msg-row user';
        div.innerHTML = `<div class="bubble">${text}</div>`;
        this.ui.feed.appendChild(div);
        this.scrollToBottom();
    }

    appendAiText(text) {
        if (!this.state.currentAiBubble) {
            const div = document.createElement('div');
            div.className = 'msg-row ai';
            div.innerHTML = `<div class="bubble"></div>`;
            this.ui.feed.appendChild(div);
            this.state.currentAiBubble = div.querySelector('.bubble');
        }
        this.state.currentAiBubble.innerText += text;
        this.scrollToBottom();
    }

    showThinking() {
        this.removeThinking();
        const div = document.createElement('div');
        div.id = 'thinking';
        div.className = 'msg-row ai';
        div.innerHTML = `<div class="bubble"><div class="typing"><div class="dot"></div><div class="dot"></div><div class="dot"></div></div></div>`;
        this.ui.feed.appendChild(div);
        this.scrollToBottom();
    }

    removeThinking() { document.getElementById('thinking')?.remove(); }
    scrollToBottom() { this.ui.feed.scrollTop = this.ui.feed.scrollHeight; }
}

// Start App
const app = new SentiricApp();