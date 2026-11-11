const invoke=window.__TAURI__?.core?.invoke,$=id=>document.getElementById(id);
let state={applications:[],resources:[],providers:[],root:"",catalog:[]};
const titles={overview:"Visão geral",apps:"Aplicativos",apis:"Provedores de API",resources:"Biblioteca de dependências"};
function note(message,error=false){const n=$("notice");n.textContent=message;n.classList.remove("hidden","error");if(error)n.classList.add("error");clearTimeout(note.timer);note.timer=setTimeout(()=>n.classList.add("hidden"),6500)}
async function act(command,args={},message){try{await invoke(command,args);if(message)note(message);await load()}catch(e){note(String(e),true);throw e}}
function esc(v=""){return String(v).replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]))}
function empty(t){return '<div class="empty">'+esc(t)+'</div>'}
function page(p){document.querySelectorAll(".page").forEach(x=>x.classList.add("hidden"));$("page-"+p).classList.remove("hidden");$("page-title").textContent=titles[p];document.querySelectorAll(".nav").forEach(x=>x.classList.toggle("active",x.dataset.page===p))}
function delay(ms){return new Promise(resolve=>setTimeout(resolve,ms))}
function setRegistrationProgress(value,title,detail){$("registration-progress").classList.remove("hidden");$("progress-fill").style.width=Math.max(0,Math.min(100,value))+"%";$("progress-percent").textContent=Math.round(value)+"%";$("progress-title").textContent=title;$("progress-detail").textContent=detail}
function showQueueResults(status){const box=$("queue-results"),list=$("queue-result-list");box.classList.remove("hidden");const results=status.results||[];list.innerHTML=results.map(r=>'<div class="queue-result"><span class="queue-dot '+(r.status==="installed"?"success":"failed")+'"></span><div><strong>'+esc(r.id)+'</strong><small>'+(r.status==="installed"?"Instalado ou já disponível":esc(r.message||"Falha na instalação"))+'</small></div><b>'+(r.status==="installed"?"Concluído":"Erro")+'</b></div>').join("")||empty("Nenhuma dependência processada.");}
function appOpts(){return state.applications.map(a=>'<option value="'+esc(a.id)+'">'+esc(a.name)+' ('+esc(a.id)+')</option>').join("")}
async function load(){if(!invoke){note("Abra o Shared Runtime Manager pelo executável desktop.",true);return}try{const [dashboard,catalog]=await Promise.all([invoke("dashboard_data"),invoke("provider_catalog")]);state={...dashboard,catalog:catalog.providers||[]};render()}catch(e){note(String(e),true)}}
function render(){
 $("root-path").textContent=state.root;
 $("stats").innerHTML=[["Aplicativos",state.applications.length,"projetos cadastrados"],["Provedores de API",state.providers.length,"configurações globais"],["Recursos",state.resources.length,"instalações compartilhadas"]].map(x=>'<div class="stat"><small>'+x[0]+'</small><strong>'+x[1]+'</strong><span>'+x[2]+'</span></div>').join("");
 $("overview-apps").innerHTML=state.applications.length?state.applications.slice(0,4).map(a=>'<div class="compact"><strong>'+esc(a.name)+'</strong><small>'+esc(a.id)+'</small></div>').join(""):empty("Nenhum aplicativo cadastrado.");
 $("overview-apis").innerHTML=state.providers.length?state.providers.slice(0,4).map(p=>'<div class="compact"><strong>'+esc(p.name)+'</strong><small>'+p.authorized_apps.length+' acesso(s)</small></div>').join(""):empty("Nenhum provedor configurado.");
 $("apps-list").innerHTML=state.applications.length?state.applications.map(a=>'<article class="entity"><div class="entity-head"><div><div class="entity-name">'+esc(a.name)+'</div><div class="entity-id">'+esc(a.id)+'</div></div><button class="small-btn danger" data-delete-app="'+esc(a.id)+'">Excluir vínculo</button></div><div class="meta">Cadastrado em '+new Date(a.registered_at*1000).toLocaleDateString()+' · '+a.resources.length+' recurso(s) vinculado(s)<br>Instalação: '+esc(a.installation_path||"Caminho não configurado")+'<br>Configuração: '+esc(a.config_path||"Ainda não gerada")+'</div><div class="chips">'+(a.resources.length?a.resources.map(r=>'<span class="chip">'+esc(r)+'</span>').join(""):'<span class="chip">Nenhum recurso vinculado</span>')+'</div></article>').join(""):empty("Cadastre seu primeiro aplicativo.");
 $("apis-list").innerHTML=state.providers.length?state.providers.map(p=>'<article class="entity"><div class="entity-head"><div><div class="entity-name">'+esc(p.name)+'</div><div class="entity-id">'+esc(p.id)+' · '+esc(p.model)+'</div><div class="meta">'+esc(p.endpoint)+'</div></div><div class="entity-actions"><button class="small-btn" data-key="'+esc(p.id)+'">Atualizar chave</button><button class="small-btn" data-models="'+esc(p.id)+'">↻ Modelos</button><button class="small-btn" data-grant="'+esc(p.id)+'">＋ Autorizar</button></div></div><div class="chips">'+(p.authorized_apps.length?p.authorized_apps.map(a=>'<span class="chip good">'+esc(a)+' <button data-provider="'+esc(p.id)+'" data-app="'+esc(a)+'" title="Revogar" style="border:0;background:none;color:inherit;cursor:pointer">×</button></span>').join(""):'<span class="chip">Sem aplicativos autorizados</span>')+'</div></article>').join(""):empty("Nenhum provedor cadastrado.");
 $("resources-list").innerHTML=state.resources.length?state.resources.map(r=>'<article class="entity"><div class="entity-name">'+esc(r.id.toUpperCase())+'</div><div class="meta">Versão: '+esc(r.version)+'<br>Local: '+esc(r.install_path)+'</div></article>').join(""):empty("Nenhum recurso instalado.");

}
document.querySelectorAll(".nav").forEach(x=>x.addEventListener("click",()=>page(x.dataset.page)));
document.querySelectorAll("[data-go]").forEach(x=>x.addEventListener("click",()=>page(x.dataset.go)));
$("refresh").onclick=load;$("add-app").onclick=()=>$("app-dialog").showModal();$("add-api").onclick=()=>$("api-dialog").showModal();
document.querySelectorAll("[data-close]").forEach(x=>x.onclick=() => $(x.dataset.close).close());
$("app-form").onsubmit=async e=>{
 e.preventDefault();const form=e.currentTarget,f=new FormData(form),submit=$("register-submit");submit.disabled=true;
 try{
  setRegistrationProgress(5,"Registrando projeto","Validando a pasta e criando shared.config.json.");
  await invoke("register_application",{appId:f.get("app_id"),name:f.get("name"),installationPath:f.get("installation_path")});
  setRegistrationProgress(15,"Projeto registrado","Configuração criada. Preparando a fila de dependências.");
  await invoke("start_dependency_queue");
  let status;
  for(let attempt=0;attempt<360;attempt++){
   status=await invoke("dependency_queue_status");
   const p=Number(status.progress||0);
   setRegistrationProgress(15+p*.85,status.status==="completed"?"Processamento concluído":"Instalando dependências",status.current||"Aguardando...");
   if(status.status==="completed"||status.status==="error")break;
   await delay(500);
  }
  if(!status||!["completed","error"].includes(status.status))throw new Error("O processamento ainda está em andamento. Consulte os resultados na tela de Aplicativos.");
  showQueueResults(status);await load();form.reset();
  if(status.status==="error"){note("O projeto foi cadastrado, mas a fila encontrou um erro: "+(status.error||"erro desconhecido"),true);$("app-dialog").close();return}
  const failed=(status.results||[]).filter(x=>x.status!=="installed").length;
  note(failed?("Projeto cadastrado. "+failed+" dependência(s) falharam e podem ser revisadas nos resultados."):"Projeto cadastrado e dependências processadas com sucesso.",failed>0);
  $("app-dialog").close();
 }catch(error){note("O projeto pode ter sido cadastrado, mas ocorreu um erro: "+String(error),true);await load()}
 finally{submit.disabled=false;setRegistrationProgress(0,"Preparando projeto...","Criando configuração compartilhada.");$("registration-progress").classList.add("hidden")}
};
$("provider-preset").onchange=()=>{const p=state.catalog.find(x=>x.id===$("provider-preset").value);const f=$("api-form");if(!p)return;f.elements.provider.value=p.id;f.elements.name.value=p.name;f.elements.endpoint.value=p.endpoint;const select=$("api-model");select.innerHTML=(p.models||[]).map(id=>"<option value=\""+esc(id)+"\">"+esc(id)+"</option>").join("");select.value=p.defaultModel||p.models?.[0]||""};
$("api-form").onsubmit=async e=>{e.preventDefault();const f=new FormData(e.currentTarget);await act("save_api_provider",{provider:f.get("provider"),name:f.get("name"),endpoint:f.get("endpoint"),model:f.get("model")},"Provedor salvo. Agora armazene a chave para consultar os modelos disponíveis.");$("api-dialog").close();e.currentTarget.reset();$("provider-preset").value=""};
$("models-form").onsubmit=async e=>{e.preventDefault();const f=new FormData(e.currentTarget);await act("save_api_provider",{provider:f.get("provider"),name:f.get("name"),endpoint:f.get("endpoint"),model:f.get("model")},"Modelo padrão atualizado.");$("models-dialog").close()};
$("key-form").onsubmit=async e=>{e.preventDefault();const f=new FormData(e.currentTarget);await act("save_api_key",{provider:f.get("provider"),secret:f.get("secret")},"Chave enviada ao Gerenciador de Credenciais do Windows.");$("key-dialog").close();e.currentTarget.reset()};
$("grant-form").onsubmit=async e=>{e.preventDefault();const f=new FormData(e.currentTarget);await act("authorize_api",{provider:f.get("provider"),appId:f.get("app_id")},"Permissão concedida.");$("grant-dialog").close()};
$("apps-list").onclick=async e=>{const b=e.target.closest("[data-delete-app]");if(!b)return;const id=b.dataset.deleteApp;if(!confirm("Remover "+id+" do Shared Runtime? A pasta e os arquivos do projeto serão preservados; somente o vínculo e shared.config.json serão removidos."))return;try{await act("delete_application",{appId:id},"Vínculo removido. Os arquivos do projeto foram preservados.");}catch{}};
$("apis-list").onclick=async e=>{const k=e.target.closest("[data-key]");if(k){$("key-form").elements.provider.value=k.dataset.key;$("key-dialog").showModal();return}const m=e.target.closest("[data-models]");if(m){const p=state.providers.find(x=>x.id===m.dataset.models);try{const models=await invoke("available_api_models",{provider:p.id});const select=$("available-models");select.innerHTML=models.map(id=>"<option value=\""+esc(id)+"\">"+esc(id)+"</option>").join("");const f=$("models-form");f.elements.provider.value=p.id;f.elements.name.value=p.name;f.elements.endpoint.value=p.endpoint;select.value=p.model;if(!models.includes(p.model))select.value=models[0];$("models-hint").textContent=models.length+" modelos retornados pelo provedor.";$("models-dialog").showModal()}catch(err){note(String(err),true)}return}const g=e.target.closest("[data-grant]");if(g){if(!state.applications.length)return note("Cadastre um aplicativo antes de conceder acesso.",true);$("grant-form").elements.provider.value=g.dataset.grant;$("grant-form").elements.app_id.innerHTML=appOpts();$("grant-dialog").showModal();return}const r=e.target.closest("[data-provider]");if(r&&confirm("Revogar acesso de "+r.dataset.app+" ao provedor "+r.dataset.provider+"?"))await act("revoke_api",{provider:r.dataset.provider,appId:r.dataset.app},"Acesso revogado.")};

$("copy-root").onclick=async()=>{try{await navigator.clipboard.writeText(state.root);note("Caminho copiado.")}catch{note("Não foi possível copiar o caminho.",true)}};
load();