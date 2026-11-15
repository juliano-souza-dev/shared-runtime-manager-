const invoke=window.__TAURI__?.core?.invoke,$=id=>document.getElementById(id);
let state={applications:[],resources:[],providers:[],root:"",catalog:[],ecosystem:[],voices:{default_voice_id:null,voices:[]}};
let processingStatus={status:"idle",progress:0,current:"Nenhuma operação executada",results:[]};
let processingLogs=[];
let lastVoiceProcessingStatus="";
let monitoredProcesses=[];
let terminalLines={};
let terminalListenerReady=false;
const titles={overview:"Visão geral",ecosystem:"Ecossistema de aplicativos",apps:"Aplicativos registrados",apis:"Provedores de API",resources:"Biblioteca de dependências",voices:"Biblioteca de vozes",processing:"Central de processamentos",monitor:"Monitor de aplicações"};
function note(message,error=false){const n=$("notice");n.textContent=message;n.classList.remove("hidden","error");if(error)n.classList.add("error");clearTimeout(note.timer);note.timer=setTimeout(()=>n.classList.add("hidden"),6500)}
async function act(command,args={},message){try{await invoke(command,args);if(message)note(message);await load()}catch(e){note(String(e),true);throw e}}
function esc(v=""){return String(v).replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]))}
function empty(t){return '<div class="empty">'+esc(t)+'</div>'}
function formatBytes(value){const bytes=Math.max(0,Number(value)||0);if(bytes<1024)return bytes+" B";const units=["KB","MB","GB","TB"];let size=bytes/1024,index=0;while(size>=1024&&index<units.length-1){size/=1024;index++}return size.toFixed(size>=100?0:1)+" "+units[index]}
function isManagedSharedPath(path){const normalize=value=>{let result=String(value||"").replaceAll("\\","/").toLowerCase();while(result.endsWith("/"))result=result.slice(0,-1);return result};const root=normalize(state.root),candidate=normalize(path);return !!root&&(candidate===root||candidate.startsWith(root+"/"))}
function page(p){document.querySelectorAll(".page").forEach(x=>x.classList.add("hidden"));$("page-"+p).classList.remove("hidden");$("page-title").textContent=titles[p];document.querySelectorAll(".nav").forEach(x=>x.classList.toggle("active",x.dataset.page===p));if(p==="processing")refreshProcessing();if(p==="monitor")refreshMonitor()}
function delay(ms){return new Promise(resolve=>setTimeout(resolve,ms))}

function showQueueResults(status){const box=$("queue-results"),list=$("queue-result-list");box.classList.remove("hidden");const results=status.results||[];list.innerHTML=results.map(r=>'<div class="queue-result"><span class="queue-dot '+(r.status==="installed"?"success":"failed")+'"></span><div><strong>'+esc(r.id)+'</strong><small>'+(r.status==="installed"?"Instalado ou já disponível":esc(r.message||"Falha na instalação"))+'</small></div><b>'+(r.status==="installed"?"Concluído":"Erro")+'</b></div>').join("")||empty("Nenhuma dependência processada.");}
function appOpts(){return state.applications.map(a=>'<option value="'+esc(a.id)+'">'+esc(a.name)+' ('+esc(a.id)+')</option>').join("")}
async function load(){if(!invoke){note("Abra o Shared Runtime Manager pelo executável desktop.",true);return}try{const [dashboard,catalog,voices,ecosystem]=await Promise.all([invoke("dashboard_data"),invoke("provider_catalog"),invoke("voice_library"),invoke("applications_catalog")]);state={...dashboard,catalog:catalog.providers||[],voices,ecosystem};render();await refreshProcessing();await refreshMonitor();setupTerminalListener()}catch(e){note(String(e),true)}}
function renderProcessing(status){
 if(status)processingStatus=status;
 const p=processingStatus||{};
 const running=["queued","installing","running","cancelling"].includes(p.status);
 const hasFailures=p.status==="error"||(p.results||[]).some(item=>!["installed","completed","cancelled"].includes(item.status));
 const badge=$("processing-status-badge");
 if(!badge)return;
 badge.textContent=p.status==="cancelling"?"Cancelamento solicitado":running?"Processamento em andamento":p.status==="cancelled"?"Cancelado":hasFailures?"Concluído com falhas":p.status==="completed"?"Concluído":"Sem tarefas ativas";
 badge.className="processing-badge "+(running?"running":p.status==="cancelled"?"cancelled":hasFailures?"failed":p.status==="completed"?"completed":"idle");
 const count=$("processing-nav-count");
 if(count){count.textContent=running?"1":"";count.classList.toggle("hidden",!running)}
 const determinate=p.progressMode==="determinate"&&Number.isFinite(Number(p.progress));
 const percent=determinate?Math.max(0,Math.min(100,Number(p.progress))):0;
 const progressLabel=determinate?percent+"%":"Acompanhando atividade real";
 const trackClass=running&&!determinate?"indeterminate":"";
 const downloadInfo=p.downloadedBytes!==undefined?'<div class="processing-download"><span>Recebido: <strong>'+formatBytes(p.downloadedBytes)+'</strong></span><span>Velocidade: <strong>'+formatBytes(p.speedBytesPerSecond||0)+'/s</strong></span></div>':"";
 const controls=running?'<div class="processing-actions"><button class="small-btn danger" data-cancel-processing '+(p.status==="cancelling"?"disabled":"")+'>Cancelar operação</button></div>':(["cancelled","error"].includes(p.status)&&p.taskType!=="voice-profile"?'<div class="processing-actions"><button class="primary" data-restart-processing>↻ Reiniciar operação</button></div>':"");
 $("processing-current").innerHTML='<div class="processing-card"><div class="processing-card-top"><div><small>OPERAÇÃO ATUAL</small><h3>'+esc(p.current||"Nenhuma operação executada")+'</h3><p>'+esc(p.taskType==="voice-profile"?"Biblioteca de vozes · Qwen3-TTS Base":p.application?"Aplicativo: "+p.application:p.resourceId?"Recurso: "+p.resourceId:"Aguardando uma operação do Shared")+'</p></div><strong class="'+(determinate?"":"progress-indeterminate-label")+'">'+progressLabel+'</strong></div><div class="processing-track"><i class="'+trackClass+'" style="width:'+percent+'%"></i></div>'+downloadInfo+'<div class="processing-meta"><span>'+esc(p.status||"idle")+'</span><span>'+(p.completed??0)+' / '+(p.total??0)+' dependências</span></div>'+controls+'</div>';
 const results=p.results||[];
 $("processing-results").innerHTML=results.length?results.map(x=>'<div class="queue-result"><span class="queue-dot '+(["installed","completed"].includes(x.status)?"success":"failed")+'"></span><div><strong>'+esc(x.id)+'</strong><small>'+(["installed","completed"].includes(x.status)?esc(x.message||"Concluído"):esc(x.message||"Falha na operação"))+'</small></div><b>'+(["installed","completed"].includes(x.status)?"Concluído":x.status==="cancelled"?"Cancelado":"Erro")+'</b></div>').join(""):empty("Os detalhes aparecerão aqui quando uma operação iniciar.");
 const history=p.history||[];
 $("processing-history").innerHTML=history.length?history.map(item=>'<div class="queue-result"><span class="queue-dot '+(item.status==="completed"?"success":"failed")+'"></span><div><strong>'+esc(item.taskType==="voice-profile"?"Criação de perfil de voz":item.resourceId||item.application||"Fila de dependências")+'</strong><small>'+esc(item.current||item.status)+' · '+(item.finishedAt?new Date(item.finishedAt*1000).toLocaleString():"")+'</small></div><b>'+ (item.status==="completed"?"Concluído":item.status==="cancelled"?"Cancelado":"Falha")+'</b></div>').join(""):empty("As operações concluídas, canceladas ou com falha aparecerão aqui.");
}
function renderProcessingLogs(logs){
 const nextLogs=Array.isArray(logs)?logs:[];
 const count=$("processing-logs-count"),list=$("processing-logs");
 if(!count||!list)return;
 if(list.dataset.rendered==="true"&&JSON.stringify(nextLogs)===JSON.stringify(processingLogs))return;
 processingLogs=nextLogs;
 list.dataset.rendered="true";
 count.textContent=processingLogs.length+" registro(s)";
 count.className="processing-badge "+(processingLogs.length?"failed":"idle");
 list.innerHTML=processingLogs.length?processingLogs.map(log=>{
  const date=log.timestamp?new Date(log.timestamp*1000).toLocaleString():"Data não informada";
  const report=[log.title||"Falha de processamento",date,"Tipo: "+(log.taskType||"não informado"),"",log.detail||"Sem detalhes."].join("\n");
  return '<article class="failure-log"><div class="failure-log-head"><div><strong>'+esc(log.title||"Falha de processamento")+'</strong><small>'+esc(date)+' · '+esc(log.taskType||"processamento")+'</small></div><button class="small-btn" data-copy-log="'+esc(log.id)+'">▣ Copiar log</button></div><details><summary>Ver relatório completo</summary><pre>'+esc(log.detail||"Sem detalhes.")+'</pre></details><textarea class="log-copy-source" data-log-report="'+esc(log.id)+'" hidden>'+esc(report)+'</textarea></article>';
 }).join(""):empty("Nenhuma falha registrada. Quando um processamento falhar, o relatório técnico aparecerá aqui.");
}
async function refreshProcessing(){if(!invoke)return;try{const [next,logs]=await Promise.all([invoke("dependency_queue_status"),invoke("processing_logs")]);renderProcessing(next);renderProcessingLogs(logs);if(next.taskType==="voice-profile"&&["completed","error"].includes(next.status)&&lastVoiceProcessingStatus!==next.status){lastVoiceProcessingStatus=next.status;if(next.status==="completed"){state.voices=await invoke("voice_library");renderVoices();note("Perfil de voz pronto e disponível na biblioteca.")}}else if(next.taskType==="voice-profile"&&next.status==="running"){lastVoiceProcessingStatus="running"}}catch(e){console.error("Não foi possível atualizar a Central de Processamentos:",e)}}
function appendTerminalLine(payload){
 const id=payload.appId;if(!id)return;
 terminalLines[id]=terminalLines[id]||[];
 terminalLines[id].push({stream:payload.stream||"stdout",line:payload.line||"",timestamp:payload.timestamp||0});
 if(terminalLines[id].length>500)terminalLines[id].splice(0,terminalLines[id].length-500);
 const output=document.querySelector('[data-terminal-output="'+CSS.escape(id)+'"]');
 if(output){const nearBottom=output.scrollHeight-output.scrollTop-output.clientHeight<32;output.insertAdjacentHTML("beforeend",'<div class="terminal-line '+esc(payload.stream||"stdout")+'">'+esc(payload.line||"")+'</div>');if(nearBottom)output.scrollTop=output.scrollHeight}
}
async function setupTerminalListener(){
 if(terminalListenerReady||!window.__TAURI__?.event?.listen)return;
 terminalListenerReady=true;
 await window.__TAURI__.event.listen("srm-terminal-output",event=>appendTerminalLine(event.payload||{}));
}
async function refreshMonitor(){
 if(!invoke)return;
 try{const [processes,catalog]=await Promise.all([invoke("application_processes"),invoke("applications_catalog")]);monitoredProcesses=processes;state.ecosystem=catalog;if(!document.activeElement?.closest("[data-terminal-form]"))renderMonitor();else{const count=$("monitor-nav-count");if(count){count.textContent=monitoredProcesses.length;count.classList.toggle("hidden",!monitoredProcesses.length)}}}catch(error){note("Não foi possível consultar os processos: "+String(error),true)}
}
function renderMonitor(){
 const list=$("monitor-list");if(!list)return;
 const previousScroll={};list.querySelectorAll("[data-terminal-output]").forEach(output=>previousScroll[output.dataset.terminalOutput]=output.scrollTop);
 const count=$("monitor-nav-count");if(count){count.textContent=monitoredProcesses.length;count.classList.toggle("hidden",!monitoredProcesses.length)}
 const apps=state.ecosystem||[];
 const activeIds=new Set(monitoredProcesses.map(p=>p.appId));
 const available=apps.filter(a=>a.installed&&a.mode==="dev");
 const cards=available.map(app=>{
  const process=monitoredProcesses.find(p=>p.appId===app.id);
  const lines=terminalLines[app.id]||[];
  const output=lines.map(line=>'<div class="terminal-line '+esc(line.stream)+'">'+esc(line.line)+'</div>').join("");
  return '<article class="panel monitor-card"><div class="monitor-card-head"><div><h3>'+esc(app.name)+'</h3><small>'+esc(app.id)+' · '+esc(app.source_path||"")+'</small></div><span class="processing-badge '+(process?'running':'idle')+'">'+(process?'DEV ativo · PID '+process.pid:'Parado')+'</span></div><div class="terminal-window"><div class="terminal-title"><span></span><span></span><span></span><b>'+esc(app.id)+' / bash</b></div><div class="terminal-output" data-terminal-output="'+esc(app.id)+'">'+(output||'<div class="terminal-placeholder">A saída do Bash aparecerá aqui quando a aplicação for iniciada.</div>')+'</div></div><div class="monitor-actions">'+(process?'<button class="small-btn danger" data-stop-process="'+esc(app.id)+'">■ Encerrar processo</button>':'<button class="primary" data-start-process="'+esc(app.id)+'">▶ Iniciar em DEV</button>')+'<button class="small-btn" data-clear-terminal="'+esc(app.id)+'">Limpar console</button></div><form class="terminal-command" data-terminal-form="'+esc(app.id)+'"><span>$</span><input name="command" autocomplete="off" spellcheck="false" placeholder="Digite um comando Bash..."><button class="small-btn">Executar</button></form></article>';
 }).join("");
 list.innerHTML=cards||empty("Nenhum aplicativo instalado em modo DEV. Instale um aplicativo pelo Ecossistema para gerenciá-lo aqui.");
 list.querySelectorAll("[data-terminal-output]").forEach(output=>{if(Object.prototype.hasOwnProperty.call(previousScroll,output.dataset.terminalOutput))output.scrollTop=previousScroll[output.dataset.terminalOutput]});
}
$("monitor-list").addEventListener("click",async e=>{
 const start=e.target.closest("[data-start-process]"),stop=e.target.closest("[data-stop-process]"),clear=e.target.closest("[data-clear-terminal]");
 if(clear){terminalLines[clear.dataset.clearTerminal]=[];renderMonitor();return}
 const button=start||stop;if(!button)return;button.disabled=true;
 try{if(start){await invoke("open_ecosystem_application",{appId:start.dataset.startProcess});note("Processo DEV iniciado pelo Bash.")}else{await invoke("stop_application_process",{appId:stop.dataset.stopProcess});note("Processo encerrado.")}await refreshMonitor()}
 catch(error){note(String(error),true)}finally{button.disabled=false}
});
$("monitor-list").addEventListener("submit",async e=>{
 const form=e.target.closest("[data-terminal-form]");if(!form)return;e.preventDefault();
 const input=form.elements.command,command=input.value.trim();if(!command)return;
 input.value="";
 try{await invoke("run_application_shell_command",{appId:form.dataset.terminalForm,command})}
 catch(error){note(String(error),true)}
});
function renderVoices(){
 const list=$("voice-list");if(!list)return;
 const voices=state.voices?.voices||[];
 if(!voices.length){list.innerHTML=empty("Nenhuma voz criada. Instale o Qwen3-TTS Base e prepare seu primeiro perfil.");return}
 list.innerHTML=voices.map(v=>{
  const isDefault=state.voices.default_voice_id===v.id;
  return '<article class="voice-item"><div class="voice-item-head"><div><strong>'+esc(v.name)+'</strong><small>'+esc(v.id)+' · '+Number(v.duration_seconds||0).toFixed(1)+'s de referência</small></div>'+(isDefault?'<span class="chip good">Padrão</span>':'<button class="small-btn" data-default-voice="'+esc(v.id)+'">Definir como padrão</button>')+'</div><div class="meta">Modelo: Qwen3-TTS 1.7B Base<br>Perfil: '+esc(v.profile_path)+'</div><div class="voice-preview" id="voice-preview-'+esc(v.id)+'"></div><div class="voice-actions"><button class="small-btn" data-preview-voice="'+esc(v.id)+'">▶ Gerar prévia</button></div></article>';
 }).join("");
}
function renderEcosystem(){
 const list=$("ecosystem-list");if(!list)return;
 list.innerHTML=(state.ecosystem||[]).map(app=>{
  const installed=app.installed;
  const mode=app.mode==="dev"?"Modo DEV":app.mode==="executable"?"Executável":"Ainda não instalado";
  const actions=installed?'<button class="small-btn primary" data-open-ecosystem="'+esc(app.id)+'">▶ Abrir</button><button class="small-btn" data-install-mode="dev" data-app-id="'+esc(app.id)+'">Reinstalar DEV</button><button class="small-btn" data-install-mode="executable" data-app-id="'+esc(app.id)+'">Gerar executável</button>':'<button class="small-btn" data-install-mode="dev" data-app-id="'+esc(app.id)+'">Instalar DEV</button><button class="small-btn primary" data-install-mode="executable" data-app-id="'+esc(app.id)+'">Gerar e instalar</button>';
  return '<article class="entity ecosystem-card"><div class="entity-head"><div><div class="entity-name">'+esc(app.name)+'</div><div class="entity-id">'+esc(app.id)+' · '+esc(app.branch)+'</div></div><span class="processing-badge '+(installed?'completed':'idle')+'">'+(installed?esc(mode):'Disponível no catálogo')+'</span></div><p class="ecosystem-description">'+esc(app.description)+'</p><div class="meta">Repositório: <a href="'+esc(app.repository)+'" target="_blank" rel="noopener">'+esc(app.repository)+'</a>'+(app.source_path?'<br>Código-fonte: '+esc(app.source_path):'')+(app.launch_path?'<br>Executável: '+esc(app.launch_path):'')+'</div><div class="entity-actions ecosystem-actions">'+actions+'</div></article>';
 }).join("")||empty("Nenhum aplicativo cadastrado no catálogo.");
}
function render(){
 $("root-path").textContent=state.root;
 renderVoices();renderEcosystem();
 $("stats").innerHTML=[["Aplicativos",state.applications.length,"projetos cadastrados"],["Provedores de API",state.providers.length,"configurações globais"],["Recursos",state.resources.length,"instalações compartilhadas"]].map(x=>'<div class="stat"><small>'+x[0]+'</small><strong>'+x[1]+'</strong><span>'+x[2]+'</span></div>').join("");
 $("overview-apps").innerHTML=state.applications.length?state.applications.slice(0,4).map(a=>'<div class="compact"><strong>'+esc(a.name)+'</strong><small>'+esc(a.id)+'</small></div>').join(""):empty("Nenhum aplicativo cadastrado.");
 $("overview-apis").innerHTML=state.providers.length?state.providers.slice(0,4).map(p=>'<div class="compact"><strong>'+esc(p.name)+'</strong><small>'+p.authorized_apps.length+' acesso(s)</small></div>').join(""):empty("Nenhum provedor configurado.");
 $("apps-list").innerHTML=state.applications.length?state.applications.map(a=>'<article class="entity"><div class="entity-head"><div><div class="entity-name">'+esc(a.name)+'</div><div class="entity-id">'+esc(a.id)+'</div></div><div class="entity-actions"><button class="small-btn" data-sync-app="'+esc(a.id)+'">↻ Sincronizar dependências</button><button class="small-btn danger" data-delete-app="'+esc(a.id)+'">Excluir vínculo</button></div></div><div class="meta">Cadastrado em '+new Date(a.registered_at*1000).toLocaleDateString()+' · '+a.resources.length+' dependência(s) declarada(s)/vinculada(s)<br>Instalação: '+esc(a.installation_path||"Caminho não configurado")+'<br>Configuração: '+esc(a.config_path||"Ainda não gerada")+'</div><div class="chips">'+(a.resources.length?a.resources.map(r=>'<span class="chip">'+esc(r)+'</span>').join(""):'<span class="chip">Nenhum recurso vinculado</span>')+'</div></article>').join(""):empty("Cadastre seu primeiro aplicativo.");
 $("apis-list").innerHTML=state.providers.length?state.providers.map(p=>'<article class="entity"><div class="entity-head"><div><div class="entity-name">'+esc(p.name)+'</div><div class="entity-id">'+esc(p.id)+' · '+esc(p.model)+'</div><div class="meta">'+esc(p.endpoint)+'</div></div><div class="entity-actions"><button class="small-btn" data-key="'+esc(p.id)+'">Atualizar chave</button><button class="small-btn" data-models="'+esc(p.id)+'">↻ Modelos</button><button class="small-btn" data-grant="'+esc(p.id)+'">＋ Autorizar</button></div></div><div class="chips">'+(p.authorized_apps.length?p.authorized_apps.map(a=>'<span class="chip good">'+esc(a)+' <button data-provider="'+esc(p.id)+'" data-app="'+esc(a)+'" title="Revogar" style="border:0;background:none;color:inherit;cursor:pointer">×</button></span>').join(""):'<span class="chip">Sem aplicativos autorizados</span>')+'</div></article>').join(""):empty("Nenhum provedor cadastrado.");
 $("resources-list").innerHTML=state.resources.length?state.resources.map(r=>{const users=state.applications.filter(a=>a.resources.includes(r.id));const managed=isManagedSharedPath(r.install_path);const action=users.length?'<button class="small-btn danger" disabled title="Dependência vinculada a um ou mais aplicativos">Em uso · '+users.length+' aplicativo(s)</button>':managed?'<button class="small-btn danger" data-delete-resource="'+esc(r.id)+'">Excluir dependência</button>':'<button class="small-btn" disabled title="O Shared não pode apagar instalações externas">Instalação externa</button>';return '<article class="entity"><div class="entity-head"><div><div class="entity-name">'+esc(r.id.toUpperCase())+'</div><div class="meta">Versão: '+esc(r.version)+'<br>Local: '+esc(r.install_path)+'</div></div>'+action+'</div>'+(users.length?'<div class="chips">'+users.map(a=>'<span class="chip good">'+esc(a.name)+'</span>').join("")+'</div>':'')+'</article>'}).join(""):empty("Nenhum recurso instalado.");

 document.querySelectorAll("[data-install-resource]").forEach(button=>{const installed=state.resources.some(resource=>resource.id===button.dataset.installResource);button.disabled=installed||["queued","installing","running","cancelling"].includes(processingStatus.status);if(["qwen3-tts","qwen3-tts-base-clone"].includes(button.dataset.installResource))button.hidden=installed;});
}
$("processing-logs").addEventListener("click",async e=>{
 const button=e.target.closest("[data-copy-log]");if(!button)return;
 const source=$("processing-logs").querySelector('[data-log-report="'+CSS.escape(button.dataset.copyLog)+'"]');
 if(!source)return;
 try{
  await navigator.clipboard.writeText(source.value);
  note("Log completo copiado para a área de transferência.");
 }catch(error){
  const temporary=document.createElement("textarea");temporary.value=source.value;document.body.appendChild(temporary);temporary.select();
  const copied=document.execCommand("copy");temporary.remove();
  if(copied)note("Log completo copiado para a área de transferência.");else note("Não foi possível copiar o log: "+String(error),true);
 }
});
$("processing-current").addEventListener("click",async e=>{
 const cancel=e.target.closest("[data-cancel-processing]");
 const restart=e.target.closest("[data-restart-processing]");
 if(!cancel&&!restart)return;
 const button=cancel||restart;button.disabled=true;
 try{
  if(cancel){await invoke("cancel_processing");note("Cancelamento solicitado. A operação será interrompida com segurança.");}
  else{await invoke("restart_processing");note("Operação reiniciada. O progresso será atualizado aqui.");}
  await refreshProcessing();
 }catch(error){note(String(error),true);button.disabled=false}
});
$("ecosystem-list").addEventListener("click",async e=>{
 const open=e.target.closest("[data-open-ecosystem]");
 if(open){open.disabled=true;try{const app=state.ecosystem.find(item=>item.id===open.dataset.openEcosystem);await invoke("open_ecosystem_application",{appId:open.dataset.openEcosystem});if(app?.mode==="dev"){note("Aplicação DEV iniciada ou já ativa no Bash integrado.");page("monitor")}else{note("Aplicativo executável iniciado.")}}catch(error){note(String(error),true)}finally{open.disabled=false}return}
 const install=e.target.closest("[data-install-mode]");if(!install)return;
 const appId=install.dataset.appId,mode=install.dataset.installMode;
 const label=mode==="dev"?"modo DEV":"modo Executável";
 if(!confirm("Instalar "+appId+" em "+label+"? O Shared fará Git Clone/atualização, instalará as dependências declaradas, gerará shared.config.json e executará as etapas necessárias."))return;
 install.disabled=true;try{await invoke("install_ecosystem_application",{appId,mode});await refreshProcessing();page("processing");note("Instalação de "+appId+" iniciada. Acompanhe cada etapa na Central de Processamentos.")}catch(error){note(String(error),true);install.disabled=false}
});
document.querySelectorAll(".nav").forEach(x=>x.addEventListener("click",()=>page(x.dataset.page)));
document.querySelectorAll("[data-go]").forEach(x=>x.addEventListener("click",()=>page(x.dataset.go)));
$("refresh").onclick=load;$("add-app").onclick=()=>$("app-dialog").showModal();$("add-api").onclick=()=>$("api-dialog").showModal();
document.querySelectorAll("[data-close]").forEach(x=>x.onclick=() => $(x.dataset.close).close());
$("page-resources").addEventListener("click",async e=>{const button=e.target.closest("[data-install-resource]");if(!button)return;const resourceId=button.dataset.installResource;const detail=resourceId==="qwen3-tts"?" O modelo CustomVoice ocupa vários GB e será executado somente em CPU.":resourceId==="qwen3-tts-base-clone"?" O modelo Base 1.7B ocupa vários GB, além do PyTorch CPU, e será usado para preparar perfis de clonagem de voz.":resourceId==="piper-tts"?" Também será baixada uma voz inglesa de aproximadamente 63 MB.":"";if(!confirm("Instalar "+resourceId+" no diretório compartilhado?"+detail+" A instalação será executada em segundo plano."))return;button.disabled=true;try{await invoke("install_shared_resource",{resourceId,appId:null});await refreshProcessing();page("processing");note("Instalação de "+resourceId+" iniciada. Acompanhe o andamento na Central de Processamentos.")}catch(error){note(String(error),true);button.disabled=false}});
$("app-form").onsubmit=async e=>{
 e.preventDefault();const form=e.currentTarget,f=new FormData(form),submit=$("register-submit");submit.disabled=true;
 try{
  await invoke("register_application",{appId:f.get("app_id"),name:f.get("name"),installationPath:f.get("installation_path")});
  await invoke("start_dependency_queue");
  form.reset();$("app-dialog").close();await load();page("processing");
  note("Projeto cadastrado. A instalação das dependências está sendo executada em segundo plano.");
 }catch(error){note("Não foi possível iniciar o cadastro/processamento: "+String(error),true);await load()}
 finally{submit.disabled=false}
};
$("provider-preset").onchange=()=>{const p=state.catalog.find(x=>x.id===$("provider-preset").value);const f=$("api-form");if(!p)return;f.elements.provider.value=p.id;f.elements.name.value=p.name;f.elements.endpoint.value=p.endpoint;$("provider-help").innerHTML='Autenticação: '+(p.auth==="query"?"chave de API na query string":p.auth==="anthropic"?"chave de API + versão Anthropic":"Bearer token")+' · <a href="'+esc(p.keyUrl)+'" target="_blank" rel="noopener">Criar chave</a> · <a href="'+esc(p.docsUrl)+'" target="_blank" rel="noopener">Documentação oficial</a>';const select=$("api-model");select.innerHTML=(p.models||[]).map(id=>"<option value=\""+esc(id)+"\">"+esc(id)+"</option>").join("");select.value=p.defaultModel||p.models?.[0]||""};
$("api-form").onsubmit=async e=>{e.preventDefault();const f=new FormData(e.currentTarget);await act("save_api_provider",{provider:f.get("provider"),name:f.get("name"),endpoint:f.get("endpoint"),model:f.get("model")},"Provedor salvo. Agora armazene a chave para consultar os modelos disponíveis.");$("api-dialog").close();e.currentTarget.reset();$("provider-preset").value=""};
$("models-form").onsubmit=async e=>{e.preventDefault();const f=new FormData(e.currentTarget);await act("save_api_provider",{provider:f.get("provider"),name:f.get("name"),endpoint:f.get("endpoint"),model:f.get("model")},"Modelo padrão atualizado.");$("models-dialog").close()};
$("key-form").onsubmit=async e=>{e.preventDefault();const f=new FormData(e.currentTarget);await act("save_api_key",{provider:f.get("provider"),secret:f.get("secret")},"Chave protegida e atualizada nos arquivos das aplicações autorizadas.");$("key-dialog").close();e.currentTarget.reset()};
$("grant-form").onsubmit=async e=>{e.preventDefault();const f=new FormData(e.currentTarget);await act("authorize_api",{provider:f.get("provider"),appId:f.get("app_id")},"Permissão concedida.");$("grant-dialog").close()};
$("apps-list").onclick=async e=>{const sync=e.target.closest("[data-sync-app]");if(sync){const id=sync.dataset.syncApp;sync.disabled=true;try{await invoke("synchronize_application",{appId:id});await refreshProcessing();page("processing");note("Sincronização de "+id+" iniciada. Acompanhe o andamento na Central de Processamentos.")}catch(err){note(String(err),true)}finally{sync.disabled=false}return}const b=e.target.closest("[data-delete-app]");if(!b)return;const id=b.dataset.deleteApp;if(!confirm("Remover "+id+" do Shared Runtime? A pasta e os arquivos do projeto serão preservados; somente o vínculo e shared.config.json serão removidos."))return;try{await act("delete_application",{appId:id},"Vínculo removido. Os arquivos do projeto foram preservados.");}catch{}};
$("resources-list").onclick=async e=>{const b=e.target.closest("[data-delete-resource]");if(!b)return;const id=b.dataset.deleteResource;if(!confirm("Excluir "+id+" do diretório central? O Shared só removerá arquivos gerenciados por ele. Se algum aplicativo ainda declarar essa dependência, a operação será bloqueada."))return;try{await act("delete_shared_dependency",{dependencyId:id},"Dependência removida da biblioteca.")}catch{}};
$("apis-list").onclick=async e=>{const k=e.target.closest("[data-key]");if(k){$("key-form").elements.provider.value=k.dataset.key;$("key-dialog").showModal();return}const m=e.target.closest("[data-models]");if(m){const p=state.providers.find(x=>x.id===m.dataset.models);try{const models=await invoke("available_api_models",{provider:p.id});const select=$("available-models");select.innerHTML=models.map(id=>"<option value=\""+esc(id)+"\">"+esc(id)+"</option>").join("");const f=$("models-form");f.elements.provider.value=p.id;f.elements.name.value=p.name;f.elements.endpoint.value=p.endpoint;select.value=p.model;if(!models.includes(p.model))select.value=models[0];$("models-hint").textContent=models.length+" modelos retornados pelo provedor.";$("models-dialog").showModal()}catch(err){note(String(err),true)}return}const g=e.target.closest("[data-grant]");if(g){if(!state.applications.length)return note("Cadastre um aplicativo antes de conceder acesso.",true);$("grant-form").elements.provider.value=g.dataset.grant;$("grant-form").elements.app_id.innerHTML=appOpts();$("grant-dialog").showModal();return}const r=e.target.closest("[data-provider]");if(r&&confirm("Revogar acesso de "+r.dataset.app+" ao provedor "+r.dataset.provider+"?"))await act("revoke_api",{provider:r.dataset.provider,appId:r.dataset.app},"Acesso revogado.")};

$("copy-root").onclick=async()=>{try{await navigator.clipboard.writeText(state.root);note("Caminho copiado.")}catch{note("Não foi possível copiar o caminho.",true)}};
$("voice-form").onsubmit=async e=>{
 e.preventDefault();const form=e.currentTarget;const data=new FormData(form);const file=data.get("audio");const submit=$("voice-submit");
 if(!file||!file.size)return note("Selecione um arquivo de áudio.",true);
 submit.disabled=true;submit.textContent="Enviando para processamento...";
 try{
  lastVoiceProcessingStatus="";
  const audioBase64=await new Promise((resolve,reject)=>{const reader=new FileReader();reader.onload=()=>resolve(String(reader.result).split(",")[1]||"");reader.onerror=()=>reject(new Error("Não foi possível ler o arquivo de áudio."));reader.readAsDataURL(file)});
  await invoke("create_voice_profile",{name:data.get("name"),filename:file.name,audioBase64,referenceText:data.get("reference_text")});
  form.reset();await refreshProcessing();page("processing");note("Preparação da voz iniciada. Acompanhe o andamento e o resultado na Central de Processamentos.");
 }catch(error){note(String(error),true)}
 finally{submit.disabled=false;submit.textContent="Preparar perfil de voz"}
};
$("voice-list").onclick=async e=>{
 const set=e.target.closest("[data-default-voice]");
 if(set){try{await invoke("set_default_voice",{voiceId:set.dataset.defaultVoice});await load();note("Voz padrão atualizada nos aplicativos registrados.")}catch(error){note(String(error),true)}return}
 const preview=e.target.closest("[data-preview-voice]");
 if(preview){preview.disabled=true;preview.textContent="Gerando...";
  try{const path=await invoke("preview_voice",{voiceId:preview.dataset.previewVoice,text:"Olá! Esta é uma prévia da minha voz sintetizada pelo Shared Runtime."});const url=window.__TAURI__.core.convertFileSrc(path);$("voice-preview-"+preview.dataset.previewVoice).innerHTML='<audio controls src="'+esc(url)+'"></audio>'}
  catch(error){note(String(error),true)}
  finally{preview.disabled=false;preview.textContent="▶ Gerar prévia"}
 }
};
$("voices-refresh").onclick=load;
$("processing-refresh").onclick=refreshProcessing;
$("monitor-refresh").onclick=refreshMonitor;
load();
setInterval(refreshProcessing,1000);
setInterval(refreshMonitor,2000);