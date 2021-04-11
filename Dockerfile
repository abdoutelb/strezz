FROM ubuntu:20.04

RUN  apt-get update  && apt-get  install -y apt-transport-https \
    && apt-get  install -y curl \
    && apt-get  install -y python3-pip python3-dev build-essential \
    &&  pip3 install locust
# Bundle app source

RUN curl --silent --location https://deb.nodesource.com/setup_14.x | bash -
RUN apt-get install --yes nodejs

# this is the build args parameter should be the same name
ARG url    
ENV SITE_URL=${url}

COPY . .

RUN npm install 

RUN url=$SITE_URL node index.js

EXPOSE 4000


CMD locust -f /loc.py --host=$SITE_URL -P 4000
#For debugging
#ENTRYPOINT ["tail", "-f", "/dev/null"]
