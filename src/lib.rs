#![recursion_limit = "256"]

use wasm_bindgen::prelude::*;
use yew::prelude::*;

use anyhow::Error;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use yew::format::Json;
use yew::services::websocket::{WebSocketStatus, WebSocketTask};
use yew::services::{ConsoleService, DialogService, WebSocketService};

use chrono::serde::ts_milliseconds;
use chrono::{DateTime, Utc};

struct ApiKey(String);

#[derive(Deserialize, Serialize, Hash, PartialEq, Eq, Clone, Debug)]
struct Symbol(String);

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, PartialOrd)]
struct Price(f32);

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, PartialOrd)]
struct Volume(f32);

/// This is a single Stock info payload that comes from the FinnPub API
#[derive(Deserialize, Serialize, Debug)]
struct TickerInfo {
    #[serde(rename = "s")]
    symbol: Symbol,
    #[serde(rename = "p")]
    price: Price,
    #[serde(rename = "v")]
    volume: Volume,
    #[serde(with = "ts_milliseconds", rename = "t")]
    time: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Request {
    Subscribe { symbol: Symbol },
    Unsubscribe { symbol: Symbol },
}

/// The different messages that we'll get from the websocket connection
#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type", rename_all = "lowercase")]
enum WsMessage {
    Error {
        #[serde(rename = "msg")]
        message: String,
    },
    Ping,
    Trade {
        data: Vec<TickerInfo>,
    },
}

#[derive(Deserialize, Serialize)]
struct TickerHistory {
    symbol_to_history: HashMap<Symbol, VecDeque<TickerInfo>>,
}

impl TickerHistory {
    const MAX_HISTORY: usize = 25;

    fn new() -> TickerHistory {
        TickerHistory {
            symbol_to_history: HashMap::new(),
        }
    }

    fn get(&self, symbol: &Symbol) -> Option<&VecDeque<TickerInfo>> {
        self.symbol_to_history.get(symbol)
    }

    fn insert(&mut self, ticker_info: TickerInfo) {
        let symbol = ticker_info.symbol.clone();
        match self.symbol_to_history.entry(symbol) {
            Entry::Occupied(mut existing) => {
                let queue = existing.get_mut();
                queue.push_front(ticker_info);
                if queue.len() > Self::MAX_HISTORY {
                    queue.pop_back();
                }
            }
            Entry::Vacant(vacant) => {
                let mut new_queue = VecDeque::new();
                new_queue.push_front(ticker_info);
                vacant.insert(new_queue);
            }
        }
    }

    fn remove(&mut self, symbol: &Symbol) {
        self.symbol_to_history.remove(symbol);
    }
}

#[derive(Deserialize, Serialize)]
struct State {
    tracked: Vec<Symbol>,
    history: TickerHistory,
}

struct UntrackResult {
    is_last: bool,
    symbol: Symbol,
}

impl State {
    fn add_symbol(&mut self, symbol: Symbol) {
        self.tracked.push(symbol);
    }

    fn last_added(&self) -> Option<&Symbol> {
        self.tracked.last()
    }

    fn remove_last_added(&mut self) {
        self.tracked.pop();
    }

    fn untrack_symbol(&mut self, idx: usize) -> UntrackResult {
        let removed_symbol = self.tracked.remove(idx);
        let last_for_symbol = self
            .tracked
            .iter()
            .find(|t| t == &&removed_symbol)
            .is_none();
        if last_for_symbol {
            self.history.remove(&removed_symbol);
        }
        UntrackResult {
            is_last: last_for_symbol,
            symbol: removed_symbol,
        }
    }

    fn add_history(&mut self, ticker_info: TickerInfo) {
        self.history.insert(ticker_info);
    }
}

struct Model {
    websocket_service: WebSocketService,
    dialog_service: DialogService,
    console_service: ConsoleService,
    api_key: ApiKey,
    symbol_to_add: Symbol,
    state: State,
    link: ComponentLink<Self>,
    websocket_task: Option<WebSocketTask>,
}

enum Msg {
    ApiKeyUpdate(ApiKey),
    UpdateSymbolToTrack(Symbol),
    TrackSymbol,
    ApiKeyConnect,
    ApiKeyDisconnect,
    UnTrackSymbolAtIdx(usize),
    WsIncoming(Result<WsMessage, Error>),
    WsOpened,
    WsDead,
    Nope,
}

enum TickerHealth {
    Good,
    Normal,
    Bad,
}

impl Component for Model {
    type Message = Msg;
    type Properties = ();
    fn create(_: Self::Properties, link: ComponentLink<Self>) -> Self {
        Model {
            api_key: ApiKey("".into()),
            symbol_to_add: Symbol("".into()),
            state: State {
                tracked: vec![],
                history: TickerHistory::new(),
            },
            websocket_service: WebSocketService::new(),
            dialog_service: DialogService::new(),
            console_service: ConsoleService::new(),
            link,
            websocket_task: None,
        }
    }

    fn update(&mut self, msg: Self::Message) -> ShouldRender {
        match msg {
            Msg::ApiKeyUpdate(key) => self.api_key = key,
            Msg::ApiKeyConnect => {
                return self.connect_to_api();
            }
            Msg::ApiKeyDisconnect => {
                self.websocket_task = None;
            }
            Msg::UpdateSymbolToTrack(symbol) => self.symbol_to_add = symbol,
            Msg::TrackSymbol => {
                if self.symbol_to_add.0.is_empty() {
                    return false;
                } else {
                    let symbol_to_add = self.symbol_to_add.clone();
                    self.state.add_symbol(symbol_to_add.clone());
                    self.symbol_to_add = Symbol("".into());
                    if let Some(websocket_task) = &mut self.websocket_task {
                        let subscribe = Request::Subscribe {
                            symbol: symbol_to_add,
                        };
                        websocket_task.send(Json(&subscribe));
                    }
                }
            }
            Msg::UnTrackSymbolAtIdx(idx) => {
                let result = self.state.untrack_symbol(idx);
                if result.is_last {
                    if let Some(websocket_task) = &mut self.websocket_task {
                        let unsubscribe = Request::Unsubscribe {
                            symbol: result.symbol,
                        };
                        websocket_task.send(Json(&unsubscribe));
                    }
                }
            }
            Msg::WsIncoming(data) => {
                match data {
                    Ok(ws_message) => {
                        self.console_service
                            .info(format!("Received message [{:?}]", ws_message).as_str());
                        match ws_message {
                            WsMessage::Error { message } => {
                                // assume the last tracked ticker was bad
                                if message == "Invalid symbol" {
                                    if let Some(last_added_ticker) = self.state.last_added() {
                                        let delete_last = self.dialog_service.confirm(
                                            format!("Invalid symbol detected. Do you want to untrack the last added one: [{}]", last_added_ticker.0).as_str()
                                        );
                                        if delete_last {
                                            self.state.remove_last_added();
                                        }
                                    }
                                }
                            }
                            WsMessage::Trade { data: tickers_data } => {
                                // go through each one, find the state to update and update it
                                for i in tickers_data {
                                    self.state.add_history(i);
                                }
                            }
                            WsMessage::Ping => return false,
                        }
                    }
                    Err(sucks) => {
                        self.console_service
                            .error(format!("Got some undeserialisable data [{}]", sucks).as_str());
                        return false;
                    }
                }
            }
            Msg::WsOpened => {
                // subscribe
                if let Some(websocket_task) = &mut self.websocket_task {
                    for tracked in &self.state.tracked {
                        let subscribe = Request::Subscribe {
                            symbol: tracked.clone(),
                        };
                        websocket_task.send(Json(&subscribe));
                    }
                } else {
                    // impossible,
                    self.dialog_service
                        .alert("The no websocket connection despite it being open, wtf?");
                }
                return true;
            }
            Msg::WsDead => {
                self.dialog_service
                    .alert("The Websocket connection could not be established 😞 Maybe your API key is wrong?");
                self.websocket_task = None;
            }
            Msg::Nope => (),
        }
        true
    }

    fn change(&mut self, _props: Self::Properties) -> ShouldRender {
        // Should only return "true" if new properties are different to
        // previously received properties.
        // This component has no properties so we will always return "false".
        false
    }

    fn view(&self) -> Html {
        html! {
        < div class = "container-fluid text-center" >
            < div class ="row" >
                < div class ="col text-center" >
                    < h1 class = "display-3">{ "finnhub trades" }< / h1 >
                < /div >
            < /div>
            < div class ="row" >
                < div class ="offset-md-4 col-md-4" >
                    { self.view_api_key_input() }
                    { self.view_ticker_input() }
                < /div >
            < /div>
            <div class = "row" >
                < div class ="offset-md-2 col-md-8" >
                        { for self.state.tracked.iter().enumerate().map( | e | self.view_symbol(e)) }
                < /div>
            < /div>
        < / div >
        }
    }
}

impl Model {
    fn connect_to_api(&mut self) -> bool {
        let callback = self.link.callback(|Json(data)| Msg::WsIncoming(data));

        let notification = self.link.callback(|status| match status {
            WebSocketStatus::Opened => Msg::WsOpened,
            WebSocketStatus::Closed | WebSocketStatus::Error => Msg::WsDead,
        });

        let websocket_task_result = self.websocket_service.connect(
            format!("wss://ws.finnhub.io?token={}", self.api_key.0).as_str(),
            callback,
            notification,
        );
        match websocket_task_result {
            Ok(websocket_task) => {
                self.websocket_task = Some(websocket_task);
                true
            }
            Err(yikes) => {
                self.dialog_service.alert(yikes);
                false
            }
        }
    }

    fn view_api_key_input(&self) -> Html {
        let ws_connected = self.websocket_task.is_some();
        let button_class = if ws_connected {
            "btn btn-secondary"
        } else {
            "btn btn-primary"
        };
        let button_text = if ws_connected {
            "Disconnect"
        } else {
            "Connect"
        };
        let button_onclick = if ws_connected {
            self.link.callback(|_| Msg::ApiKeyDisconnect)
        } else {
            self.link.callback(|_| Msg::ApiKeyConnect)
        };

        let button_icon = if ws_connected {
            html! {
            <i class="fas fa-unlink" style="color:red;"></i>
            }
        } else {
            html! {
            <i class="fas fa-link"></i>
            }
        };

        html! {
        <div class="input-group mb-3">
          <input
            type="text"
            class="form-control"
            placeholder="API Key"
            aria-label="API Key"
            aria-describedby="api-key-connect"
            value =& self.api_key.0
            oninput = self.link.callback( | e: InputData | Msg::ApiKeyUpdate(ApiKey(e.value)))
            onkeypress = self.link.callback( |e: KeyboardEvent | {
                if e.key() == "Enter" { Msg::ApiKeyConnect } else { Msg::Nope }
            })
            disabled=ws_connected
            />
          <div class="input-group-append">
            <button class=button_class
             type="button"
             id="api-key-connect"
             aria-label={ button_text }
             onclick=button_onclick>
                 { button_icon }
            </button>
          </div>
        </div>
        }
    }

    fn view_ticker_input(&self) -> Html {
        html! {
        <div class="input-group mb-3">
          <input
            type="text"
            class="form-control"
            placeholder="Ticker symbol"
            aria-label="Ticker symbol"
            aria-describedby="track-symbol"
            value =& self.symbol_to_add.0
            oninput = self.link.callback( | e: InputData | Msg::UpdateSymbolToTrack(Symbol(e.value)))
            onkeypress = self.link.callback( |e: KeyboardEvent | {
                if e.key() == "Enter" { Msg::TrackSymbol } else { Msg::Nope }
            })
            />
          <div class="input-group-append">
            <button class="btn btn-success"
             type="button"
             id="track-symbol"
             onclick=self.link.callback( | _ | Msg::TrackSymbol )>
                 <i class="fas fa-plus-circle"></i>
            </button>
          </div>
        </div>
        }
    }

    fn view_ticker_info_row(&self, ticker_info: &TickerInfo) -> Html {
        html! {
            <tr>
              <td>{ ticker_info.time }</td>
              <td>{ ticker_info.volume.0 }</td>
              <td>{ ticker_info.price.0 }</td>
            </tr>
        }
    }

    fn view_symbol(&self, (idx, symbol): (usize, &Symbol)) -> Html {
        let maybe_symbol_history = self.state.history.get(symbol);

        let mut ticker_health = TickerHealth::Normal;

        let last_trade_details = if let Some(symbol_history) = maybe_symbol_history {
            if let (Some(last_trade), Some(second_last)) =
                (symbol_history.get(0), symbol_history.get(1))
            {
                if last_trade.price > second_last.price {
                    ticker_health = TickerHealth::Good;
                } else if last_trade.price < second_last.price {
                    ticker_health = TickerHealth::Bad;
                }
            }

            html! {
                <div class="table-responsive">
                  <table class="table table-hover">
                      <thead>
                        <tr>
                          <th scope="col">{ "Time" }</th>
                          <th scope="col">{ "Volume" }</th>
                          <th scope="col">{ "Price ($)" }</th>
                        </tr>
                      </thead>
                      <tbody class="text-right">
                        { for symbol_history.iter().map( | t | self.view_ticker_info_row(t))}
                      </tbody>
                  </table>
                </div>
            }
        } else {
            let not_connected_warning = if self.websocket_task.is_none() {
                html! {
                <p class="card-text border-warning"><small class="text-muted">{ "Not connected to API"}</small></p>
                }
            } else {
                html! {}
            };
            html! {
                <div class="text-left">
                    <p class="card-text">{ "No trades details yet" }</p>
                    { not_connected_warning }
                </div>
            }
        };

        let card_health_class = match ticker_health {
            TickerHealth::Good => "border-success",
            TickerHealth::Bad => "border-danger",
            TickerHealth::Normal => "border-primary",
        };
        let card_class = format!("card m-2 {}", card_health_class);

        html! {
        <div class={ card_class }>
          <div class="card-header">
            < div class ="d-flex w-100 justify-content-between" >
                <div class="flex-fill text-left">
                    <h5 class="mb-1">{ & symbol.0 }</h5>
                </div>
                < div class="flex-fill text-right">
                    <button type="button" class="close" aria-label="Untrack" onclick = self.link.callback( move | _ | Msg::UnTrackSymbolAtIdx(idx)) >
                      <i class="fas fa-times"></i>
                    </button>
                </div>
            < / div >
          </div>
          <div class="card-body">
             { last_trade_details }
          </div>
        </div>
        }
    }
}

#[wasm_bindgen(start)]
pub fn run_app() {
    App::<Model>::new().mount_to_body();
}
